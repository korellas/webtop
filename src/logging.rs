//! Size-bounded log files for the launchd-managed process.
//!
//! launchd's `StandardOutPath` / `StandardErrorPath` append forever — there is
//! no built-in rotation, and `newsyslog` cannot help us here: it rotates by
//! renaming the file, but launchd hands the child an already-open descriptor.
//! After a rename that descriptor still points at the rotated-away inode, so
//! every subsequent write lands in the archived file while the fresh one stays
//! empty. The log would keep growing under a new name and we would have gained
//! nothing.
//!
//! The one operation that behaves correctly against an inherited descriptor is
//! truncating the file *in place*. launchd opens these paths with `O_APPEND`,
//! so once the inode is truncated the next write resumes at offset 0. We keep
//! the tail of the file — the part actually worth reading — and drop
//! everything before it.
//!
//! Caveat: we are also the process writing to that descriptor, so a concurrent
//! write landing between the read and the truncate is lost. For a diagnostic
//! log that trade is worth a hard size ceiling.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Rotate once a file grows past this. Two files × 4 MB is a ceiling small
/// enough to never matter and large enough to hold a meaningful crash history.
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

/// How much of the tail survives a rotation. This is what you actually read
/// when diagnosing a failure — the oldest bytes are never the interesting ones.
const KEEP_TAIL_BYTES: u64 = 512 * 1024;

/// How often the size is re-checked while running.
const CHECK_INTERVAL: Duration = Duration::from_secs(3600);

/// The two files launchd redirects our stdio into.
fn log_paths(home: &str) -> [PathBuf; 2] {
    let dir = Path::new(home).join(".webtop");
    [dir.join("webtop.out.log"), dir.join("webtop.err.log")]
}

/// Cap both log files immediately, then keep them capped hourly.
///
/// The startup pass matters most: a crash-loop is the failure mode that
/// actually bloats these files, and every restart in that loop runs this.
pub fn spawn_rotator(home: String) {
    tokio::spawn(async move {
        loop {
            let paths = log_paths(&home);
            let _ = tokio::task::spawn_blocking(move || {
                for path in &paths {
                    match rotate_if_needed(path) {
                        Ok(Some(dropped)) => {
                            tracing::info!(
                                path = %path.display(),
                                dropped_bytes = dropped,
                                "truncated oversized log"
                            );
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(
                                path = %path.display(),
                                error = %e,
                                "log rotation failed"
                            );
                        }
                    }
                }
            })
            .await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Truncate `path` to its trailing [`KEEP_TAIL_BYTES`] if it exceeds
/// [`MAX_LOG_BYTES`]. Returns the number of bytes dropped, or `None` if the
/// file was already small enough (or does not exist yet).
fn rotate_if_needed(path: &Path) -> std::io::Result<Option<u64>> {
    let len = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        // Not created yet — launchd creates it on the first write.
        Err(_) => return Ok(None),
    };
    if len <= MAX_LOG_BYTES {
        return Ok(None);
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(len - KEEP_TAIL_BYTES))?;
    let mut tail = Vec::with_capacity(KEEP_TAIL_BYTES as usize);
    file.read_to_end(&mut tail)?;
    drop(file);

    // Seeking to a byte offset almost certainly lands mid-line. Drop that
    // fragment so the file always resumes on a record boundary.
    if let Some(newline) = tail.iter().position(|&b| b == b'\n') {
        tail.drain(..=newline);
    }

    let dropped = len - tail.len() as u64;

    // Truncate in place rather than rename — see the module docs for why.
    let mut out = OpenOptions::new().write(true).open(path)?;
    out.set_len(0)?;
    out.write_all(format!("--- log truncated, dropped {dropped} bytes ---\n").as_bytes())?;
    out.write_all(&tail)?;
    out.flush()?;

    Ok(Some(dropped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("webtop-logtest-{name}-{}", std::process::id()));
        p
    }

    #[test]
    fn leaves_small_files_untouched() {
        let path = temp_path("small");
        std::fs::write(&path, b"one line\n").unwrap();

        let dropped = rotate_if_needed(&path).unwrap();

        assert_eq!(dropped, None);
        assert_eq!(std::fs::read(&path).unwrap(), b"one line\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn truncates_oversized_file_to_bounded_size() {
        let path = temp_path("big");
        // 6 MB of numbered lines — comfortably past MAX_LOG_BYTES.
        let mut content = Vec::new();
        let mut n = 0u64;
        while content.len() < 6 * 1024 * 1024 {
            content.extend_from_slice(format!("line {n}\n").as_bytes());
            n += 1;
        }
        let original_len = content.len() as u64;
        std::fs::write(&path, &content).unwrap();

        let dropped = rotate_if_needed(&path).unwrap().expect("should rotate");

        let after = std::fs::read(&path).unwrap();
        assert!(dropped > 0);
        assert!(
            (after.len() as u64) < MAX_LOG_BYTES,
            "post-rotation size {} should be under the {MAX_LOG_BYTES} ceiling",
            after.len()
        );
        assert!((after.len() as u64) < original_len);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn keeps_the_newest_lines_and_starts_on_a_boundary() {
        let path = temp_path("tail");
        let mut content = Vec::new();
        let mut n = 0u64;
        while content.len() < 6 * 1024 * 1024 {
            content.extend_from_slice(format!("line {n}\n").as_bytes());
            n += 1;
        }
        let last_line = format!("line {}\n", n - 1);
        std::fs::write(&path, &content).unwrap();

        rotate_if_needed(&path).unwrap().expect("should rotate");

        let after = String::from_utf8(std::fs::read(&path).unwrap()).unwrap();
        assert!(after.starts_with("--- log truncated"));
        assert!(after.ends_with(&last_line), "newest line must survive");
        // Every retained line is whole: the marker plus complete records.
        for line in after.lines().skip(1) {
            assert!(line.starts_with("line "), "found a partial line: {line:?}");
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let path = temp_path("absent");
        std::fs::remove_file(&path).ok();
        assert_eq!(rotate_if_needed(&path).unwrap(), None);
    }
}
