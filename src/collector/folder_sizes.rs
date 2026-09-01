//! Directory-size scanner for the Disk drawer's "largest folders" view.
//!
//! One traversal produces the size AND file count of every directory in the
//! tree. Both matter: size answers "what is big", file count predicts what a
//! re-scan of that subtree will cost, which is what lets the drawer refresh
//! cheap folders on open and leave expensive ones on cache.
//!
//! Those two axes are not correlated. Measured on the author's home directory:
//!
//! | folder      |    size | files   | rescan |
//! |-------------|---------|---------|--------|
//! | ai          | 204.2 G |  24 469 |  2.30s |
//! | git         |  32.4 G | 979 969 | 13.03s |
//! | Downloads   |   2.8 G |     663 |  0.01s |
//!
//! The largest folder was among the cheapest to verify; the most expensive one
//! was a sixth its size. Cost tracks inode count, not bytes.
//!
//! Accounting rules, chosen to match `du` so the numbers reconcile with what
//! users see in a terminal:
//!   - `st_blocks * 512`, not `st_size`. APFS clones and sparse files make the
//!     logical size a lie; allocated blocks are what the volume actually gives up.
//!   - Hard links counted once, via a (device, inode) set.
//!   - Symlinks are never followed — they would double-count and can form cycles.
//!   - The walk stays on one device.
//!   - macOS's two cloud-provider areas are never entered — see
//!     [`is_cloud_provider_area`], which is the one exclusion that is about
//!     safety rather than accounting.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Directories smaller than this are dropped. They can never surface in a
/// "largest folders" view, and keeping them would turn ~4.5k rows into ~300k.
pub const MIN_SIZE_BYTES: u64 = 10 * 1024 * 1024;

/// Guards against pathological nesting (and any cycle a bind mount could
/// introduce despite the symlink rule). Real trees are nowhere near this.
const MAX_DEPTH: u32 = 64;

/// The two directories under `~/Library` that macOS reserves for File Provider
/// content: `CloudStorage` (Synology Drive, Dropbox, OneDrive, Google Drive)
/// and `Mobile Documents` (iCloud Drive).
///
/// These are excluded because entering one can hang the scanner permanently.
/// `opendir` on a provider domain blocks in the kernel until the extension
/// answers, and if the extension is itself waiting on a TCC consent dialog
/// nobody clicks, it never answers. **No deadline inside this walk can escape
/// that**: [`Walk::out_of_time`] is only consulted between directories, so a
/// `read_dir` that never returns takes the whole traversal with it — and, one
/// level up, the scan coordinator's claim along with it.
///
/// The exclusion costs less than it looks. A provider's files are dataless
/// placeholders whose `st_blocks` is zero, so they were never going to appear
/// in a "largest folders" list; worse, enumerating a domain can prompt the
/// provider to materialise — that is, download — everything it lists.
///
/// Detection is by path because that is what the platform actually guarantees.
/// `st_dev` does not work: a domain reports the same device as the volume that
/// hosts it (measured 16777229 for both), so the one-device rule sails past it.
fn is_cloud_provider_area(dir: &Path) -> bool {
    if dir.parent().and_then(Path::file_name) != Some("Library".as_ref()) {
        return false;
    }
    matches!(
        dir.file_name().and_then(|n| n.to_str()),
        Some("CloudStorage" | "Mobile Documents")
    )
}

/// How long a directory gets to open before it is called blocked.
///
/// A healthy `opendir` is a handful of microseconds even on cold cache, so this
/// is four orders of magnitude of headroom. Anything past it is not a slow disk,
/// it is a TCC consent dialog nobody is going to click.
const OPEN_TIMEOUT: Duration = Duration::from_secs(2);

/// After this many blocked children, the parent is written off wholesale.
///
/// `~/Library/Containers` holds 646 entries on this machine and
/// `Group Containers` another 118, every one of them individually protected.
/// Probing each costs two seconds and one permanently blocked thread, so
/// learning them one at a time would cost 25 minutes and 764 threads. Three
/// consecutive refusals from one parent is enough to conclude the whole
/// directory is closed to us.
///
/// Deliberately *learned* rather than hardcoded: with Full Disk Access granted
/// nothing times out, nothing collapses, and those 764 directories are measured
/// normally. A static exclusion list would keep skipping them forever.
const BLOCKED_SIBLINGS_BEFORE_COLLAPSE: u32 = 3;

/// Opens directories on a thread the walk is willing to abandon.
///
/// `opendir` against a TCC-protected directory blocks in the kernel until the
/// consent dialog is answered — there is no timeout, no signal and no
/// cancellation that brings the thread back. Calling it inline therefore bets
/// the entire traversal on every directory being openable, and losing that bet
/// once froze this feature for two days.
///
/// So the call happens on a helper thread and the walk waits [`OPEN_TIMEOUT`].
/// On expiry the helper is abandoned — it exits by itself if the dialog is ever
/// answered — a fresh one takes over, and the walk continues with the next
/// directory. The unit of loss is one directory instead of the whole walk.
///
/// The helper is reused across every directory that opens normally, so the cost
/// in the common case is one channel round trip (a few microseconds) against a
/// walk that already takes minutes.
struct DirOpener {
    helper: Option<Helper>,
    /// Threads left blocked in `opendir`. Reported so the cost is visible.
    abandoned: u32,
}

struct Helper {
    requests: mpsc::Sender<PathBuf>,
    replies: mpsc::Receiver<std::io::Result<std::fs::ReadDir>>,
}

impl Helper {
    fn spawn(background: bool) -> Helper {
        let (requests, inbox) = mpsc::channel::<PathBuf>();
        let (outbox, replies) = mpsc::channel();

        std::thread::spawn(move || {
            // The helper now issues every `opendir`, so it — not the walker —
            // is the thread whose I/O priority matters.
            if background {
                lower_current_thread_priority();
            }
            for path in inbox {
                // A send error means the walker gave up on us. Nothing left to
                // serve, so retire rather than hold the channel open.
                if outbox.send(std::fs::read_dir(&path)).is_err() {
                    return;
                }
            }
        });

        Helper { requests, replies }
    }
}

impl DirOpener {
    fn new() -> Self {
        DirOpener {
            helper: None,
            abandoned: 0,
        }
    }

    /// Open `dir`, or `None` if it refused or never answered.
    fn open(&mut self, dir: &Path, background: bool) -> Option<std::fs::ReadDir> {
        // Lazily spawned so a walk that opens nothing costs no thread.
        let helper = self.helper.get_or_insert_with(|| Helper::spawn(background));

        if helper.requests.send(dir.to_path_buf()).is_err() {
            // The helper died on its own; retry once with a fresh one rather
            // than reporting a readable directory as unreadable.
            let fresh = Helper::spawn(background);
            let sent = fresh.requests.send(dir.to_path_buf()).is_ok();
            self.helper = Some(fresh);
            if !sent {
                return None;
            }
        }

        let helper = self.helper.as_ref().expect("just installed");
        match helper.replies.recv_timeout(OPEN_TIMEOUT) {
            Ok(Ok(entries)) => Some(entries),
            // A real error: permission refused outright, or gone since we
            // listed it. Either way the directory is simply unreadable.
            Ok(Err(_)) => None,
            Err(_) => {
                // Blocked, or the helper vanished mid-request. Its thread is
                // stuck in a syscall nothing can interrupt, so let it go.
                self.helper = None;
                self.abandoned += 1;
                None
            }
        }
    }
}

/// One directory's accounting, covering its entire subtree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSize {
    pub path: PathBuf,
    pub parent: PathBuf,
    /// Allocated bytes of everything beneath this directory.
    pub size_bytes: u64,
    /// Files beneath this directory. Drives the re-scan cost estimate.
    pub file_count: u64,
    /// Entries we could not stat or descend into — almost always macOS TCC
    /// protection on Desktop/Documents/Downloads. Surfaced so the UI can say
    /// "partially unreadable" instead of quietly reporting a number that is
    /// too small.
    pub unreadable: u64,
}

/// Result of one traversal.
#[derive(Debug, Default)]
pub struct ScanResult {
    /// Every directory at or above [`MIN_SIZE_BYTES`], unordered.
    pub folders: Vec<FolderSize>,
    /// Wall-clock time the traversal took.
    pub elapsed: Duration,
    /// True if a deadline cut the walk short; totals are then lower bounds.
    pub truncated: bool,
    /// Directories that never answered, plus any parent collapsed under
    /// [`BLOCKED_SIBLINGS_BEFORE_COLLAPSE`]. Feeding these back as `skip` on
    /// the next walk is what keeps the cost a one-off rather than per-scan.
    pub blocked: HashSet<PathBuf>,
    /// Threads left blocked in `opendir` by this walk. Never reclaimed.
    pub abandoned_threads: u32,
}

/// Running totals for one directory, returned up the recursion.
#[derive(Default, Clone, Copy)]
struct Subtotal {
    size_bytes: u64,
    file_count: u64,
    unreadable: u64,
}

impl Subtotal {
    fn absorb(&mut self, other: Subtotal) {
        self.size_bytes += other.size_bytes;
        self.file_count += other.file_count;
        self.unreadable += other.unreadable;
    }
}

/// Drop this thread to macOS's background QoS tier, where the scheduler
/// throttles its I/O so it cannot compete with foreground work.
///
/// This is the same mechanism as `taskpolicy -b`. Measured cost of the
/// throttle on a 2.9M-file home directory: 20s -> 47s wall clock, with no
/// perceptible impact on other applications. The trade is worth it for a
/// task nobody is waiting on.
pub fn lower_current_thread_priority() {
    // SAFETY: setpriority with PRIO_DARWIN_THREAD applies to the calling
    // thread and takes no pointers. A failure is not actionable — we would
    // simply run at normal priority — so the return value is ignored.
    unsafe {
        libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG);
    }
}

/// Walk `root` and size every directory beneath it.
///
/// `deadline` bounds the walk. On expiry the traversal unwinds early and
/// [`ScanResult::truncated`] is set; partial totals are still returned because
/// a lower bound beats nothing for the on-open refresh path.
pub fn scan(root: &Path, deadline: Option<Instant>) -> ScanResult {
    scan_guarded(root, deadline, &HashSet::new(), false)
}

/// [`scan`], told what a previous walk already learned.
///
/// `skip` names directories that refused to open before; they are counted as
/// unreadable without being probed again, which is what stops every scan from
/// re-paying two seconds and one lost thread apiece. Whatever this walk learns
/// comes back in [`ScanResult::blocked`].
///
/// `background` drops the thread that issues `opendir` to background I/O
/// priority — right for the periodic full scan, wrong for the drawer's refresh,
/// which has a request waiting on it.
pub fn scan_guarded(
    root: &Path,
    deadline: Option<Instant>,
    skip: &HashSet<PathBuf>,
    background: bool,
) -> ScanResult {
    let started = Instant::now();
    let mut ctx = Walk {
        folders: Vec::new(),
        seen_hardlinks: HashSet::new(),
        root_dev: std::fs::symlink_metadata(root).map(|m| m.dev()).ok(),
        deadline,
        truncated: false,
        skip,
        root,
        background,
        opener: DirOpener::new(),
        blocked: HashSet::new(),
        blocked_children: HashMap::new(),
    };

    let total = ctx.visit(root, 0);

    // The root itself is always reported, whatever its size — callers need a
    // total even for a small tree.
    ctx.folders.push(FolderSize {
        path: root.to_path_buf(),
        parent: root.parent().unwrap_or(root).to_path_buf(),
        size_bytes: total.size_bytes,
        file_count: total.file_count,
        unreadable: total.unreadable,
    });

    ScanResult {
        folders: ctx.folders,
        elapsed: started.elapsed(),
        truncated: ctx.truncated,
        blocked: ctx.blocked,
        abandoned_threads: ctx.opener.abandoned,
    }
}

/// Size a set of directories independently, cheapest first, within a budget.
///
/// This is the drawer's on-open refresh. `costs` carries the file count each
/// path had at the last full scan; ordering by it means the most folders get
/// verified per second of budget. Paths whose turn never comes are simply
/// absent from the result and keep their cached value.
pub fn scan_within_budget(
    costs: &[(PathBuf, u64)],
    budget: Duration,
) -> HashMap<PathBuf, FolderSize> {
    scan_within_budget_guarded(costs, budget, &HashSet::new()).0
}

/// [`scan_within_budget`], told what earlier walks learned and reporting what
/// this one did. See [`scan_guarded`].
pub fn scan_within_budget_guarded(
    costs: &[(PathBuf, u64)],
    budget: Duration,
    skip: &HashSet<PathBuf>,
) -> (HashMap<PathBuf, FolderSize>, HashSet<PathBuf>) {
    let deadline = Instant::now() + budget;

    let mut order: Vec<&(PathBuf, u64)> = costs.iter().collect();
    order.sort_by_key(|(_, file_count)| *file_count);

    let mut out = HashMap::new();
    let mut learned = HashSet::new();
    for (path, _) in order {
        if Instant::now() >= deadline {
            break;
        }
        // Each subtree is told what the ones before it learned, so the drawer's
        // refresh does not spend its whole budget rediscovering the same closed
        // directories.
        let mut known = skip.clone();
        known.extend(learned.iter().cloned());
        let result = scan_guarded(path, Some(deadline), &known, false);
        learned.extend(result.blocked.iter().cloned());
        // A truncated subtree would report a total that is too low. Publishing
        // it would look like the user just freed a lot of space.
        if result.truncated {
            continue;
        }
        if let Some(root_entry) = result.folders.iter().find(|f| f.path == *path) {
            out.insert(path.clone(), root_entry.clone());
        }
    }
    (out, learned)
}

struct Walk<'a> {
    folders: Vec<FolderSize>,
    /// (device, inode) of multiply-linked files already counted.
    seen_hardlinks: HashSet<(u64, u64)>,
    root_dev: Option<u64>,
    deadline: Option<Instant>,
    truncated: bool,
    /// Directories a previous walk found closed. Never probed again.
    skip: &'a HashSet<PathBuf>,
    /// The directory this walk was pointed at; never collapsed.
    root: &'a Path,
    background: bool,
    opener: DirOpener,
    /// What this walk learned, for the next one to be given as `skip`.
    blocked: HashSet<PathBuf>,
    /// Blocked children seen per parent, feeding the collapse rule.
    blocked_children: HashMap<PathBuf, u32>,
}

impl Walk<'_> {
    /// Remember a directory that would not open, and collapse to the parent
    /// once enough of its siblings have refused.
    ///
    /// The collapse is what makes this converge. Without it, a directory of 646
    /// individually-protected sandbox containers costs 646 probes and 646
    /// permanently blocked threads; with it, three.
    fn record_blocked(&mut self, dir: &Path) {
        self.blocked.insert(dir.to_path_buf());

        let Some(parent) = dir.parent() else {
            return;
        };
        // Never collapse the directory the walk was pointed at. Home has more
        // than three protected children all by itself — Desktop, Documents,
        // Downloads — so without this the very first scan would write off the
        // entire tree.
        if parent == self.root {
            return;
        }

        let refusals = self
            .blocked_children
            .entry(parent.to_path_buf())
            .or_insert(0);
        *refusals += 1;
        if *refusals == BLOCKED_SIBLINGS_BEFORE_COLLAPSE {
            self.blocked.insert(parent.to_path_buf());
        }
    }

    fn out_of_time(&mut self) -> bool {
        if self.truncated {
            return true;
        }
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                self.truncated = true;
                return true;
            }
        }
        false
    }

    /// Accumulate `dir`'s subtree, recording it if it clears the size floor.
    /// Returns the subtree's contribution to its parent.
    fn visit(&mut self, dir: &Path, depth: u32) -> Subtotal {
        let mut total = Subtotal::default();
        if depth >= MAX_DEPTH || self.out_of_time() {
            return total;
        }

        // All three checks come before the open, because the open is the call
        // that can block. `is_cloud_provider_area` is what we know in advance,
        // `skip` is what earlier walks learned, `blocked` is what this one has.
        // The parent test is what makes a collapse pay off inside this walk
        // rather than only the next one: once three of a directory's children
        // have refused, the remaining siblings are skipped without probing.
        let closed = is_cloud_provider_area(dir)
            || self.skip.contains(dir)
            || self.blocked.contains(dir)
            || dir.parent().is_some_and(|p| self.blocked.contains(p));
        if closed {
            total.unreadable += 1;
            return total;
        }

        let entries = match self.opener.open(dir, self.background) {
            Some(entries) => entries,
            // Refused or never answered. Counted so the UI can say the total is
            // a lower bound, and remembered so no later walk pays for it again.
            None => {
                self.record_blocked(dir);
                total.unreadable += 1;
                return total;
            }
        };

        for entry in entries {
            let Ok(entry) = entry else {
                total.unreadable += 1;
                continue;
            };

            // `DirEntry::metadata` issues `fstatat` against the directory's own
            // descriptor, so the kernel resolves one filename instead of the
            // whole path. Going through `symlink_metadata(entry.path())`
            // instead costs a full path walk per entry and allocates a PathBuf
            // for every file — measured at 5x the total scan time on a 2.6M
            // file tree. It has the same no-follow semantics.
            let Ok(meta) = entry.metadata() else {
                total.unreadable += 1;
                continue;
            };

            if meta.is_symlink() {
                continue;
            }

            if meta.is_dir() {
                // Do not cross onto another filesystem: a mounted volume's
                // contents belong to that volume's own total.
                if self.root_dev.is_some_and(|dev| meta.dev() != dev) {
                    continue;
                }
                // Only now is a PathBuf worth allocating.
                let path = entry.path();
                let sub = self.visit(&path, depth + 1);
                total.absorb(sub);

                if sub.size_bytes >= MIN_SIZE_BYTES {
                    self.folders.push(FolderSize {
                        path,
                        parent: dir.to_path_buf(),
                        size_bytes: sub.size_bytes,
                        file_count: sub.file_count,
                        unreadable: sub.unreadable,
                    });
                }
                continue;
            }

            // A file linked from several places must only be billed once.
            if meta.nlink() > 1 && !self.seen_hardlinks.insert((meta.dev(), meta.ino())) {
                continue;
            }

            total.size_bytes += meta.blocks() * 512;
            total.file_count += 1;
        }

        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A scratch directory that cleans itself up.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(name: &str) -> Self {
            static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut p = std::env::temp_dir();
            p.push(format!("webtop-scan-{}-{name}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempTree(p)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        /// Write `size` bytes at `rel`, creating parents as needed.
        fn file(&self, rel: &str, size: usize) -> PathBuf {
            let full = self.0.join(rel);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(&full, vec![b'x'; size]).unwrap();
            full
        }

        fn dir(&self, rel: &str) -> PathBuf {
            let full = self.0.join(rel);
            fs::create_dir_all(&full).unwrap();
            full
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn find<'a>(result: &'a ScanResult, path: &Path) -> Option<&'a FolderSize> {
        result.folders.iter().find(|f| f.path == path)
    }

    #[test]
    fn counts_every_file_in_the_subtree() {
        let t = TempTree::new("count");
        t.file("a.bin", 1024);
        t.file("sub/b.bin", 1024);
        t.file("sub/deep/c.bin", 1024);

        let result = scan(t.path(), None);
        let root = find(&result, t.path()).expect("root is always reported");

        assert_eq!(root.file_count, 3);
        assert_eq!(root.unreadable, 0);
        assert!(!result.truncated);
    }

    #[test]
    fn root_is_reported_even_when_below_the_size_floor() {
        let t = TempTree::new("tiny");
        t.file("small.bin", 16);

        let result = scan(t.path(), None);

        assert!(
            find(&result, t.path()).is_some(),
            "callers need a total for the root regardless of size"
        );
    }

    #[test]
    fn small_subdirectories_are_dropped() {
        let t = TempTree::new("floor");
        let small = t.dir("small");
        t.file("small/a.bin", 64);

        let result = scan(t.path(), None);

        assert!(
            find(&result, &small).is_none(),
            "a 64-byte directory must not occupy a row"
        );
    }

    #[test]
    fn large_subdirectories_are_recorded_with_their_parent() {
        let t = TempTree::new("big");
        let big = t.dir("big");
        t.file("big/blob.bin", (MIN_SIZE_BYTES + 4096) as usize);

        let result = scan(t.path(), None);
        let entry = find(&result, &big).expect("a directory over the floor is recorded");

        assert_eq!(entry.parent, t.path());
        assert!(entry.size_bytes >= MIN_SIZE_BYTES);
        assert_eq!(entry.file_count, 1);
    }

    #[test]
    fn sizes_roll_up_through_nesting() {
        let t = TempTree::new("rollup");
        let outer = t.dir("outer");
        let inner = t.dir("outer/inner");
        t.file("outer/inner/blob.bin", (MIN_SIZE_BYTES + 4096) as usize);

        let result = scan(t.path(), None);
        let outer_entry = find(&result, &outer).unwrap();
        let inner_entry = find(&result, &inner).unwrap();
        let root = find(&result, t.path()).unwrap();

        assert_eq!(outer_entry.size_bytes, inner_entry.size_bytes);
        assert_eq!(root.size_bytes, inner_entry.size_bytes);
        assert_eq!(root.file_count, 1);
    }

    #[test]
    fn symlinks_are_not_followed() {
        let t = TempTree::new("symlink");
        t.file("real/blob.bin", 4096);
        std::os::unix::fs::symlink(t.path().join("real"), t.path().join("link")).unwrap();

        let result = scan(t.path(), None);
        let root = find(&result, t.path()).unwrap();

        assert_eq!(
            root.file_count, 1,
            "following the link would count blob.bin twice"
        );
        assert!(find(&result, &t.path().join("link")).is_none());
    }

    #[test]
    fn hard_links_are_counted_once() {
        let t = TempTree::new("hardlink");
        let original = t.file("original.bin", 8192);
        fs::hard_link(&original, t.path().join("alias.bin")).unwrap();

        let result = scan(t.path(), None);
        let root = find(&result, t.path()).unwrap();

        assert_eq!(
            root.file_count, 1,
            "both names point at one inode holding one copy of the bytes"
        );
    }

    #[test]
    fn unreadable_directories_are_counted_not_silently_skipped() {
        let t = TempTree::new("denied");
        let locked = t.dir("locked");
        t.file("locked/hidden.bin", 4096);
        t.file("visible.bin", 4096);
        fs::set_permissions(
            &locked,
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o000),
        )
        .unwrap();

        let result = scan(t.path(), None);
        let root = find(&result, t.path()).unwrap();

        // Restore before the assert so Drop can clean up even on failure.
        let _ = fs::set_permissions(
            &locked,
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        );

        assert!(
            root.unreadable > 0,
            "a denied directory must be reported, not treated as empty"
        );
    }

    #[test]
    fn cloud_provider_areas_are_skipped_rather_than_descended() {
        let t = TempTree::new("cloud");
        // The shape macOS gives a File Provider domain, and iCloud Drive's.
        t.file(
            "Library/CloudStorage/SynologyDrive-me/placeholder.bin",
            4096,
        );
        t.file(
            "Library/Mobile Documents/com~apple~CloudDocs/note.bin",
            4096,
        );
        t.file("Library/Preferences/real.bin", 4096);

        let result = scan(t.path(), None);
        let root = find(&result, t.path()).unwrap();

        assert_eq!(
            root.file_count, 1,
            "only the genuinely local file is counted"
        );
        assert!(
            !result
                .folders
                .iter()
                .any(|f| f.path.ends_with("SynologyDrive-me")),
            "a provider domain must never be opened, let alone reported"
        );
        assert!(
            root.unreadable >= 2,
            "both skipped areas must be surfaced, not silently dropped"
        );
    }

    #[test]
    fn a_skipped_directory_is_never_opened_and_is_counted_unreadable() {
        let t = TempTree::new("skip");
        t.file("poisoned/huge.bin", 8192);
        t.file("fine/ok.bin", 4096);

        let skip = HashSet::from([t.path().join("poisoned")]);
        let result = scan_guarded(t.path(), None, &skip, false);
        let root = find(&result, t.path()).unwrap();

        assert_eq!(
            root.file_count, 1,
            "the skipped subtree contributes nothing"
        );
        assert!(root.unreadable >= 1, "the skip must be visible in the tree");
    }

    /// Make `rel` unopenable for the duration of `body`, restoring it after so
    /// the temp tree can still be cleaned up when an assertion fails.
    fn with_closed_dirs<T>(t: &TempTree, rels: &[&str], body: impl FnOnce() -> T) -> T {
        use std::os::unix::fs::PermissionsExt;
        for rel in rels {
            fs::set_permissions(t.path().join(rel), fs::Permissions::from_mode(0o000)).unwrap();
        }
        let out = body();
        for rel in rels {
            let _ = fs::set_permissions(t.path().join(rel), fs::Permissions::from_mode(0o755));
        }
        out
    }

    #[test]
    fn a_directory_that_will_not_open_is_reported_for_the_next_walk() {
        let t = TempTree::new("blocked");
        t.file("closed/hidden.bin", 4096);
        t.file("open/fine.bin", 4096);

        let result = with_closed_dirs(&t, &["closed"], || {
            scan_guarded(t.path(), None, &HashSet::new(), false)
        });

        assert!(
            result.blocked.contains(&t.path().join("closed")),
            "a walk must hand back what it could not open, or every scan pays for it again"
        );
    }

    #[test]
    fn unopenable_siblings_collapse_to_their_parent() {
        let t = TempTree::new("collapse");
        for name in ["a", "b", "c", "d"] {
            t.file(&format!("containers/{name}/x.bin"), 1024);
        }
        let closed = ["containers/a", "containers/b", "containers/c"];

        let result = with_closed_dirs(&t, &closed, || {
            scan_guarded(t.path(), None, &HashSet::new(), false)
        });

        assert!(
            result.blocked.contains(&t.path().join("containers")),
            "three refusals from one parent must write off the parent — \
             probing 646 sandbox containers one at a time costs 646 lost threads"
        );
    }

    #[test]
    fn the_scan_root_is_never_collapsed_by_its_own_children() {
        let t = TempTree::new("rootguard");
        for name in ["a", "b", "c"] {
            t.file(&format!("{name}/x.bin"), 1024);
        }

        let result = with_closed_dirs(&t, &["a", "b", "c"], || {
            scan_guarded(t.path(), None, &HashSet::new(), false)
        });

        assert!(
            !result.blocked.contains(t.path()),
            "home has more than three protected children of its own; collapsing \
             the root would discard the whole tree on the first scan"
        );
    }

    #[test]
    fn an_expired_deadline_marks_the_result_truncated() {
        let t = TempTree::new("deadline");
        t.file("a/b/c.bin", 1024);

        let result = scan(t.path(), Some(Instant::now()));

        assert!(result.truncated);
    }

    #[test]
    fn budget_scan_visits_cheapest_first() {
        let t = TempTree::new("budget");
        let cheap = t.dir("cheap");
        t.file("cheap/one.bin", 1024);
        let pricey = t.dir("pricey");
        for i in 0..50 {
            t.file(&format!("pricey/f{i}.bin"), 128);
        }

        // Ample budget: both get done, proving ordering does not drop work.
        let costs = vec![(pricey.clone(), 50_u64), (cheap.clone(), 1_u64)];
        let out = scan_within_budget(&costs, Duration::from_secs(30));

        assert!(out.contains_key(&cheap));
        assert!(out.contains_key(&pricey));
        assert_eq!(out[&cheap].file_count, 1);
        assert_eq!(out[&pricey].file_count, 50);
    }

    #[test]
    fn budget_scan_returns_nothing_when_the_budget_is_already_spent() {
        let t = TempTree::new("nobudget");
        let d = t.dir("d");
        t.file("d/a.bin", 1024);

        let out = scan_within_budget(&[(d, 1)], Duration::from_millis(0));

        assert!(
            out.is_empty(),
            "no budget means no work and no partial totals"
        );
    }
}
