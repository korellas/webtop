//! Scheduling and mutual exclusion for directory-size scans.
//!
//! Three things trigger a scan and they must not overlap — two walks of the
//! same tree would double the I/O for no benefit, and two writers racing on
//! the same rows would interleave generations:
//!
//!   1. every 6 h, after a startup delay (full tree, ~45 s at background QoS)
//!   2. the user pressing refresh (full tree)
//!   3. opening the drawer (only the visible rows, 3 s budget)
//!
//! A single [`ScanCoordinator`] serialises them. Losing the race is not an
//! error: a scan is already producing the data the caller wanted, so the caller
//! just serves cache.
//!
//! Every walk runs on a thread this module is willing to lose. See
//! [`run_off_thread`] for why that is not defensiveness but the only available
//! recovery from a `read_dir` that never returns.

use crate::collector::folder_sizes::{self, FolderSize};
use crate::storage::db::MetricsDb;
use crate::sync::guard;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How often the full tree is re-walked.
const FULL_SCAN_INTERVAL: Duration = Duration::from_secs(6 * 3600);

/// Delay before the first scan so we never compete with login and app launch.
const STARTUP_DELAY: Duration = Duration::from_secs(120);

/// Wall-clock ceiling for the drawer's on-open verification.
pub const VERIFY_BUDGET: Duration = Duration::from_secs(3);

/// How long a scan may hold the claim before it is presumed wedged.
///
/// A full walk measures 145–185 s, so this is roughly five times the worst
/// observed run. The failure it exists for is not slowness: it is a `read_dir`
/// blocked in the kernel forever on an unresponsive mount, which no deadline
/// inside the walk can interrupt. Without a handoff, one such scan retires the
/// entire feature — the timer, the refresh button and the drawer's on-open
/// verify all take the `None` branch and return silently, and the folder tree
/// stays frozen at whatever the last successful walk wrote.
const WEDGED_AFTER_MS: i64 = 15 * 60 * 1000;

/// Serialises the three scan triggers.
///
/// Claims are generation-stamped rather than a plain flag, because a scan can
/// be abandoned while still running: the thread stays blocked in the kernel and
/// its [`ScanGuard`] is dropped at some arbitrary later moment, possibly never.
/// The generation is what lets that late `Drop` recognise it no longer owns the
/// claim and leave the current holder alone.
#[derive(Default)]
pub struct ScanCoordinator {
    /// Generation of the scan holding the claim; 0 when idle.
    holder: AtomicI64,
    /// When the holder claimed, ms since epoch. Only read when `holder` is set.
    claimed_at_ms: AtomicI64,
    /// Directories a walk has already hung in. See [`quarantine`].
    ///
    /// [`quarantine`]: ScanCoordinator::quarantine
    quarantined: Mutex<HashSet<PathBuf>>,
}

impl ScanCoordinator {
    /// Directories no walk should probe again this process.
    pub fn blocked(&self) -> HashSet<PathBuf> {
        guard(self.quarantined.lock()).clone()
    }

    /// Remember directories that would not open.
    ///
    /// Carried across scans because each one costs two seconds and one
    /// permanently blocked thread to rediscover, and there can be hundreds.
    ///
    /// Deliberately not persisted to disk, though. Every cause is temporary —
    /// an unanswered consent dialog, a wedged File Provider extension, an
    /// unresponsive mount, a permission not yet granted — so a restart is the
    /// natural moment to find out whether it still applies. Writing it to the
    /// database would keep folders missing long after the cause was gone,
    /// including immediately after Full Disk Access was granted.
    pub fn remember_blocked(&self, dirs: impl IntoIterator<Item = PathBuf>) {
        guard(self.quarantined.lock()).extend(dirs);
    }
}

/// Releases the claim however the scan ends, including a panic.
pub struct ScanGuard<'a> {
    coordinator: &'a ScanCoordinator,
    generation: i64,
}

impl ScanGuard<'_> {
    /// Renew the claim.
    ///
    /// A scan that abandons a hung walk and retries can legitimately run for
    /// [`MAX_WALK_ATTEMPTS`] × [`WALK_DEADLINE`] — far past [`WEDGED_AFTER_MS`].
    /// Without this it would declare *itself* wedged and start a second scan
    /// alongside the first. Progress is what the wedge check is really asking
    /// about, and only the holder knows it is making any.
    fn heartbeat(&self) {
        self.coordinator
            .claimed_at_ms
            .store(now_ms(), Ordering::Release);
    }
}

impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        // Compare-exchange, not store: if this scan was presumed wedged its
        // claim has already been reassigned, and clearing the flag here would
        // let a third scan start alongside the one that took over.
        let _ = self.coordinator.holder.compare_exchange(
            self.generation,
            0,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

impl ScanCoordinator {
    /// Claim the right to scan, or `None` if a live scan holds it.
    pub fn try_claim(&self) -> Option<ScanGuard<'_>> {
        self.try_claim_at(now_ms())
    }

    fn try_claim_at(&self, now: i64) -> Option<ScanGuard<'_>> {
        loop {
            let holder = self.holder.load(Ordering::Acquire);
            if holder != 0 {
                let age = now.saturating_sub(self.claimed_at_ms.load(Ordering::Acquire));
                if age < WEDGED_AFTER_MS {
                    return None;
                }
                tracing::warn!(
                    held_for_s = age / 1000,
                    "folder scan appears wedged; reassigning the claim"
                );
            }

            let generation = holder.wrapping_add(1);
            if self
                .holder
                .compare_exchange(holder, generation, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.claimed_at_ms.store(now, Ordering::Release);
                return Some(ScanGuard {
                    coordinator: self,
                    generation,
                });
            }
            // Someone else moved first; re-read and decide again. Their store to
            // `claimed_at_ms` may not have landed yet, which is exactly why the
            // decision is retaken from scratch rather than reusing `holder`.
        }
    }

    pub fn is_running(&self) -> bool {
        self.holder.load(Ordering::Acquire) != 0
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// The tree we scan. Home only — see the design notes: system directories are
/// largely unreadable without Full Disk Access, and everything a user can
/// actually reclaim lives under home.
pub fn scan_root() -> PathBuf {
    PathBuf::from(crate::config::dirs_home())
}

/// Walk the whole tree and swap the result in. Blocking; call from a thread.
///
/// Returns the number of rows written, or `None` if another scan was already
/// running.
pub fn run_full_scan(db: &MetricsDb, coordinator: &ScanCoordinator) -> Option<usize> {
    let claim = coordinator.try_claim()?;
    let root = scan_root();

    // The helper thread is dropped to background QoS by `scan_guarded`, but it
    // only issues `opendir`. Every `readdir` and `fstatat` — the bulk of the
    // I/O — happens right here, so this thread needs the same treatment.
    folder_sizes::lower_current_thread_priority();
    let result = folder_sizes::scan_guarded(&root, None, &coordinator.blocked(), true);
    // The walk bounds itself now, but a machine with hundreds of closed
    // directories still spends two seconds on each the first time round. Renew
    // the claim so that long-but-healthy pass is not mistaken for a wedge.
    claim.heartbeat();

    if !result.blocked.is_empty() {
        tracing::warn!(
            directories = result.blocked.len(),
            threads_lost = result.abandoned_threads,
            "directories would not open and are excluded until webtop restarts; \
             grant Full Disk Access to measure them"
        );
        coordinator.remember_blocked(result.blocked.iter().cloned());
    }

    if result.truncated {
        // Only a deadline truncates, and a full scan sets none. Defensive:
        // publishing a truncated tree would understate every folder.
        tracing::warn!("full folder scan reported truncation; discarding");
        return Some(0);
    }

    tracing::info!(
        folders = result.folders.len(),
        elapsed_s = result.elapsed.as_secs_f32(),
        "folder scan complete"
    );

    match db.replace_folder_tree(&root, &result.folders, now_ms()) {
        Ok(written) => Some(written),
        Err(e) => {
            tracing::warn!(error = %e, "could not store folder scan");
            Some(0)
        }
    }
}

/// Re-measure `paths`, cheapest first, within [`VERIFY_BUDGET`].
///
/// This is what makes the drawer feel live. Cost tracks a folder's file count,
/// not its size, so the ordering comes from the counts recorded by the last
/// full scan — which is why we store them.
///
/// Returns the rows that were actually re-measured. Paths that did not fit in
/// the budget are absent and keep their cached value.
pub fn run_verify(
    db: &MetricsDb,
    coordinator: &ScanCoordinator,
    paths: &[String],
) -> Vec<FolderSize> {
    let Some(_guard) = coordinator.try_claim() else {
        return Vec::new();
    };

    let costs: Vec<(PathBuf, u64)> = paths
        .iter()
        .map(|p| {
            // An unrecorded path has no cost estimate. Treat it as expensive
            // so a surprise 900k-file directory cannot eat the whole budget
            // ahead of folders we know are cheap.
            let cost = db
                .folder_entry(p)
                .ok()
                .flatten()
                .map(|row| row.file_count as u64)
                .unwrap_or(u64::MAX);
            (PathBuf::from(p), cost)
        })
        .collect();

    let (measured, learned) =
        folder_sizes::scan_within_budget_guarded(&costs, VERIFY_BUDGET, &coordinator.blocked());
    coordinator.remember_blocked(learned);
    let rows: Vec<FolderSize> = measured.into_values().collect();

    if !rows.is_empty() {
        if let Err(e) = db.update_folder_rows(&rows, now_ms()) {
            tracing::warn!(error = %e, "could not store verified folder rows");
        }
    }
    rows
}

/// Walk `root` on a thread we are willing to lose.
///
/// The walk runs where it can be given up on, because `opendir` against a
/// TCC-protected directory blocks in the kernel until someone answers a consent
/// dialog. There is no timeout, no signal and no cancellation that brings that
/// thread back; the deadline inside the walk cannot help either, since it is
/// only consulted between directories. Running it inline is therefore a bet
/// that every directory under home is openable — and losing that bet once cost
/// this feature two days of frozen data.
///
/// So: on overrun the thread is abandoned (it will exit on its own if the
/// dialog is ever answered), the directory it died in is quarantined, and the
/// next scan gets past it. Returns `None` when the walk was abandoned.
/// Run a full scan on its own thread so the caller returns immediately.
pub fn spawn_full_scan(db: Arc<MetricsDb>, coordinator: Arc<ScanCoordinator>) {
    std::thread::spawn(move || {
        run_full_scan(&db, &coordinator);
    });
}

/// How long until the stored tree is old enough to justify another walk.
///
/// `Duration::ZERO` means "due now": either nothing has ever been scanned, or
/// the last completed scan is already older than [`FULL_SCAN_INTERVAL`].
///
/// A stored timestamp in the future (the clock moved backwards) is treated as
/// fresh rather than as due. The alternative — scanning immediately — turns a
/// clock adjustment into a walk on every pass of the loop.
fn time_until_due(db: &MetricsDb) -> Duration {
    let Some(last) = db.last_full_scan_at() else {
        return Duration::ZERO;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let age = Duration::from_millis(now.saturating_sub(last).max(0) as u64);
    FULL_SCAN_INTERVAL
        .checked_sub(age)
        .unwrap_or(Duration::ZERO)
}

/// Floor between two scan *attempts*.
///
/// A completed scan records its finish time, so the loop's own due-check
/// already spaces successful passes a full interval apart. This only governs
/// attempts that produced nothing — the claim was lost to a manual rescan, or
/// a truncated pass was discarded — where the due-check would still say "now"
/// and the loop would spin.
const RETRY_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// Start the periodic scanner: a pass whenever the stored tree is older than
/// [`FULL_SCAN_INTERVAL`], checked first after [`STARTUP_DELAY`].
///
/// The tree is durable, so a restart is not a reason to re-walk it. It used to
/// be: every start scanned unconditionally, which on a home directory of 2.9 M
/// files is 145 s of a core (throttled) for a result that was already in
/// SQLite and at most six hours old. With `KeepAlive` in front of the process,
/// anything that restarts it — a deploy, an OOM kill, a crash loop — paid that
/// again each time. Nobody is waiting on a live answer here: the drawer opens
/// against the cache and refreshes what it can afford within
/// [`VERIFY_BUDGET`].
pub fn spawn_periodic(db: Arc<MetricsDb>, coordinator: Arc<ScanCoordinator>) {
    std::thread::spawn(move || {
        std::thread::sleep(STARTUP_DELAY);
        loop {
            let due_in = time_until_due(&db);
            if !due_in.is_zero() {
                std::thread::sleep(due_in);
                // Re-check rather than scanning straight away: a manual
                // rescan may have refreshed the tree while we slept.
                continue;
            }
            run_full_scan(&db, &coordinator);
            std::thread::sleep(RETRY_INTERVAL);
        }
    });
}

/// Whether a path is inside the scan root. Guards the API against being
/// pointed at arbitrary filesystem locations.
///
/// `Path::starts_with` alone is not enough: it compares components literally,
/// so `/Users/me/../../etc` "starts with" `/Users/me` and would sail through.
/// The same applies to a symlink inside home pointing anywhere on the volume.
/// Both are closed by resolving the path for real and comparing the result.
pub fn is_within_root(path: &Path) -> bool {
    // Reject `..` up front, before any filesystem access. Doing it here rather
    // than relying on canonicalize keeps the check total for paths that do not
    // exist, and makes the intent obvious at the call site.
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }

    let Ok(root) = scan_root().canonicalize() else {
        return false;
    };
    // canonicalize resolves symlinks, so a link out of home resolves to its
    // real location and fails the prefix test. It also fails for paths that do
    // not exist, which is what we want — we only ever serve real directories.
    match path.canonicalize() {
        Ok(resolved) => resolved == root || resolved.starts_with(&root),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_db() -> MetricsDb {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = format!("/tmp/webtop-scan-sched-test-{}-{n}.db", std::process::id());
        let _ = std::fs::remove_file(&path);
        MetricsDb::open(&path).expect("open test db")
    }

    fn ms_now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    #[test]
    fn a_fresh_cache_defers_the_walk_across_a_restart() {
        let db = mk_db();
        // Scanned an hour ago: five of the six-hour interval are left, and a
        // process start must not spend 145 s re-deriving what is already
        // stored.
        db.meta_set(
            "folder_scan_completed_at",
            &(ms_now() - 3_600_000).to_string(),
        );

        let due_in = time_until_due(&db);
        assert!(
            due_in > Duration::from_secs(4 * 3600) && due_in < FULL_SCAN_INTERVAL,
            "expected ~5 h of the interval left, got {due_in:?}"
        );
    }

    #[test]
    fn a_stale_or_absent_cache_is_due_immediately() {
        let db = mk_db();
        assert_eq!(
            time_until_due(&db),
            Duration::ZERO,
            "a database that has never been scanned is due now"
        );

        db.meta_set(
            "folder_scan_completed_at",
            &(ms_now() - 7 * 3600 * 1000).to_string(),
        );
        assert_eq!(
            time_until_due(&db),
            Duration::ZERO,
            "older than the interval is due now"
        );
    }

    #[test]
    fn a_timestamp_from_the_future_is_treated_as_fresh() {
        let db = mk_db();
        // Clock moved backwards. Reading this as "due" would walk the tree on
        // every pass of the loop until wall-clock caught up.
        db.meta_set(
            "folder_scan_completed_at",
            &(ms_now() + 3_600_000).to_string(),
        );
        assert_eq!(time_until_due(&db), FULL_SCAN_INTERVAL);
    }

    #[test]
    fn one_claim_at_a_time() {
        let c = ScanCoordinator::default();

        let first = c.try_claim().expect("uncontended claim succeeds");
        assert!(c.is_running());
        assert!(c.try_claim().is_none(), "second claim must be refused");

        drop(first);
        assert!(!c.is_running());
        assert!(c.try_claim().is_some(), "claim is reusable once released");
    }

    #[test]
    fn a_panicking_scan_still_releases_the_claim() {
        let c = ScanCoordinator::default();

        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = c.try_claim().unwrap();
            panic!("scan blew up");
        }));

        assert!(!c.is_running(), "Drop must run on unwind");
        assert!(c.try_claim().is_some());
    }

    #[test]
    fn a_wedged_scan_hands_its_claim_to_the_next_caller() {
        let c = ScanCoordinator::default();
        let stuck = c.try_claim().expect("uncontended claim succeeds");

        let just_before = now_ms() + WEDGED_AFTER_MS - 1;
        assert!(
            c.try_claim_at(just_before).is_none(),
            "a scan still inside its allowance keeps the claim"
        );

        let past_it = now_ms() + WEDGED_AFTER_MS + 1;
        let taken_over = c
            .try_claim_at(past_it)
            .expect("a wedged scan must not disable the feature forever");

        // The abandoned scan may return at any time — even years later, when a
        // TCC prompt is finally answered. It must not clear the flag out from
        // under whoever took over.
        drop(stuck);
        assert!(
            c.is_running(),
            "the takeover still holds the claim after the wedged scan unwinds"
        );

        drop(taken_over);
        assert!(!c.is_running());
    }

    #[test]
    fn blocked_directories_survive_between_scans() {
        let c = ScanCoordinator::default();
        assert!(c.blocked().is_empty());

        c.remember_blocked([PathBuf::from("/tmp/webtop-test-closed")]);
        c.remember_blocked([PathBuf::from("/tmp/webtop-test-closed-too")]);

        let known = c.blocked();
        assert!(known.contains(Path::new("/tmp/webtop-test-closed")));
        assert!(
            known.contains(Path::new("/tmp/webtop-test-closed-too")),
            "later scans must add to what earlier ones learned, not replace it"
        );
    }

    #[test]
    fn the_root_itself_is_allowed() {
        assert!(is_within_root(&scan_root()));
    }

    #[test]
    fn a_real_directory_under_the_root_is_allowed() {
        let root = scan_root();
        let dir = root.join(format!("webtop-within-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let allowed = is_within_root(&dir);
        let _ = std::fs::remove_dir(&dir);

        assert!(allowed);
    }

    #[test]
    fn absolute_paths_outside_the_root_are_rejected() {
        assert!(!is_within_root(Path::new("/etc")));
        assert!(!is_within_root(Path::new("/System/Library")));
    }

    #[test]
    fn dot_dot_cannot_climb_out_of_the_root() {
        let root = scan_root();

        // Literal component comparison would accept every one of these: they
        // all begin with the root's components before climbing away.
        assert!(!is_within_root(&root.join("../../etc")));
        assert!(!is_within_root(&root.join("..")));
        assert!(!is_within_root(&root.join("../../../")));
    }

    #[test]
    fn a_symlink_pointing_out_of_the_root_is_rejected() {
        let root = scan_root();
        let link = root.join(format!("webtop-escape-test-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink("/etc", &link).unwrap();

        let allowed = is_within_root(&link);
        let _ = std::fs::remove_file(&link);

        assert!(
            !allowed,
            "a link inside home must not grant access to its target outside home"
        );
    }

    #[test]
    fn a_path_that_does_not_exist_is_rejected() {
        assert!(!is_within_root(&scan_root().join("no-such-directory-here")));
    }
}
