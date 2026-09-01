//! Per-process physical footprint via `proc_pid_rusage`.
//!
//! `ps rss` is the wrong number on this machine, and not by a little. RSS
//! counts pages mapped into the process's own address space; it does not count
//! memory a process owns through Metal. On Apple Silicon that is where a model
//! server keeps everything:
//!
//! | pid 28526 (model worker) | value |
//! |---|---:|
//! | `ps rss` / `ri_resident_size` | 2.3 GB |
//! | `ri_phys_footprint` | **59.8 GB** |
//!
//! Measured 2026-08-03. The services panel exists to compare a service against
//! its declared memory budget, and it was reading 2.84 GB against a 44 GB
//! budget for a process that was 16 GB *over* it. `phys_footprint` is the
//! kernel's own ownership ledger — the one jetsam kills on and the one
//! Activity Monitor and `footprint(1)` print — so it is the number the budget
//! was written against.
//!
//! **What this still cannot see.** A GGUF that is `mmap`ed and then handed to
//! Metal as a residency set can be charged to *system* wired memory and to no process
//! at all. `vmmap` shows the region as 156.9 GB virtual with 73 MB resident,
//! and `ri_phys_footprint` agrees at 9.5 GB. There is no per-process API that
//! attributes it; it appears only in the system-wide wired figure the memory
//! drawer already charts. So a service can still read far under its budget
//! while filling the machine, and the stack bar is where that gap shows up.
//!
//! **Root-owned processes are not readable.** `proc_pid_rusage` returns
//! `EPERM` for any process this uid does not own (verified against pid 1 and
//! WindowServer), which is the same restriction that made `sysinfo` unusable
//! here — see `processes.rs`. Callers fall back to the `ps` value, which is a
//! lower bound rather than a zero.

/// `RUSAGE_INFO_V2` — the oldest flavour carrying `ri_phys_footprint` that is
/// present on every macOS we support. `libc` binds the structs but not the
/// flavour constants.
const RUSAGE_INFO_V2: libc::c_int = 2;

/// The process's physical footprint in bytes, or `None` when the kernel will
/// not tell us — the process exited between enumeration and this call, or it
/// belongs to another uid.
pub fn phys_footprint(pid: u32) -> Option<u64> {
    let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
    // SAFETY: `info` is a live, correctly-sized `rusage_info_v2` and the
    // flavour we pass is the one that selects that layout, so the kernel
    // writes exactly within it. `rusage_info_t` is `void *` in the header, so
    // the buffer argument is a pointer to our pointer-sized-or-larger struct,
    // which is what the double indirection in the signature expects. Failure
    // is reported through the return value; nothing is written on error.
    let rc = unsafe {
        libc::proc_pid_rusage(
            pid as libc::c_int,
            RUSAGE_INFO_V2,
            &mut info as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
        )
    };
    if rc != 0 {
        return None;
    }
    Some(info.ri_phys_footprint)
}

/// `phys_footprint`, falling back to a caller-supplied resident-size reading.
///
/// Every caller has a `ps` row in hand already and wants the best number
/// available rather than a hole in the table, so the fallback is threaded here
/// instead of being repeated at each site.
pub fn phys_footprint_or(pid: u32, fallback_bytes: u64) -> u64 {
    phys_footprint(pid).unwrap_or(fallback_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_plausible_footprint_for_our_own_process() {
        let me = std::process::id();
        let f = phys_footprint(me).expect("a process can always read itself");
        // A running test binary is at least a megabyte and nowhere near a
        // terabyte. The point is to catch a struct-offset mistake, which is
        // the failure mode here: reading `ri_pageins` instead would land far
        // outside this range, and reading past the struct would be garbage.
        assert!(
            (1 << 20..1 << 40).contains(&f),
            "implausible footprint {f} — check the rusage_info_v2 layout"
        );
    }

    #[test]
    fn unreadable_pid_falls_back_rather_than_reporting_zero() {
        // PID 1 is launchd, owned by root; unless the tests run as root this
        // is EPERM. Either way the fallback must be what comes back when the
        // kernel declines, never a silent 0.
        let fallback = 4_096;
        assert_eq!(
            phys_footprint_or(1, fallback),
            phys_footprint(1).unwrap_or(fallback)
        );
    }

    #[test]
    fn a_pid_that_cannot_exist_yields_the_fallback() {
        // Above `kern.maxproc` ceilings by a wide margin, so there is nothing
        // to race against.
        assert_eq!(phys_footprint(u32::MAX - 1), None);
        assert_eq!(phys_footprint_or(u32::MAX - 1, 777), 777);
    }
}
