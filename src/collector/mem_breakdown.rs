//! Memory breakdown via Mach's `host_statistics64` (HOST_VM_INFO64).
//!
//! Direct FFI — avoids the mach2 crate whose surface area differs across
//! versions. We only need one call and a small struct, so wiring it by
//! hand keeps our dependencies minimal.

use crate::collector::snapshot::MemBreakdown;

const HOST_VM_INFO64: libc::c_int = 4;

/// Mirrors `struct vm_statistics64` from `<mach/vm_statistics.h>`.
/// Field order matters — it matches the C ABI exactly.
#[repr(C)]
#[derive(Default, Debug)]
struct VmStatistics64 {
    free_count: libc::c_uint,
    active_count: libc::c_uint,
    inactive_count: libc::c_uint,
    wire_count: libc::c_uint,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: libc::c_uint,
    speculative_count: libc::c_uint,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: libc::c_uint,
    throttled_count: libc::c_uint,
    external_page_count: libc::c_uint,
    internal_page_count: libc::c_uint,
    total_uncompressed_pages_in_compressor: u64,
}

extern "C" {
    fn mach_host_self() -> libc::c_uint;
    fn host_statistics64(
        host_priv: libc::c_uint,
        flavor: libc::c_int,
        host_info_out: *mut libc::c_int,
        host_info_outCnt: *mut libc::c_uint,
    ) -> libc::c_int;
}

const KERN_SUCCESS: libc::c_int = 0;

/// Return the system memory breakdown. On any failure returns a zeroed struct.
pub fn collect() -> MemBreakdown {
    let page_size = kernel_page_size();
    let host = unsafe { mach_host_self() };

    let mut info = VmStatistics64::default();
    // Count is number of `integer_t` (c_int) fields the struct contains,
    // which the kernel uses as the buffer size argument.
    let field_count = (std::mem::size_of::<VmStatistics64>() / std::mem::size_of::<libc::c_int>())
        as libc::c_uint;
    let mut count = field_count;

    let kr = unsafe {
        host_statistics64(
            host,
            HOST_VM_INFO64,
            &mut info as *mut _ as *mut libc::c_int,
            &mut count,
        )
    };
    if kr != KERN_SUCCESS {
        return MemBreakdown::default();
    }

    let pages_to_bytes = |p: u64| p.saturating_mul(page_size);

    MemBreakdown {
        wired: pages_to_bytes(info.wire_count as u64),
        active: pages_to_bytes(info.active_count as u64),
        inactive: pages_to_bytes(info.inactive_count as u64),
        compressed: pages_to_bytes(info.compressor_page_count as u64),
        free: pages_to_bytes(
            (info.free_count as u64).saturating_add(info.speculative_count as u64),
        ),
    }
}

/// Query the kernel for its native VM page size (`vm.pagesize`).
/// Defaults to 16 KiB — the Apple Silicon page size — if the sysctl fails.
fn kernel_page_size() -> u64 {
    let mut page_size: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let name = std::ffi::CString::new("vm.pagesize").unwrap();
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut page_size as *mut _ as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 && page_size > 0 {
        page_size as u64
    } else {
        16 * 1024
    }
}
