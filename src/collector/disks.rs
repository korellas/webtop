//! Mounted volume enumeration for the Disk drawer.
//!
//! Filters out Time Machine snapshots and other synthetic APFS volumes so
//! the list reflects what users actually think of as "drives".

use serde::Serialize;
use sysinfo::Disks;

#[derive(Debug, Clone, Serialize)]
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub fs_type: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub is_removable: bool,
    pub is_boot: bool,
}

pub fn list_disks() -> Vec<DiskInfo> {
    let disks = Disks::new_with_refreshed_list();
    let mut out: Vec<DiskInfo> = disks
        .list()
        .iter()
        .filter_map(|d| {
            let mount = d.mount_point().to_string_lossy().into_owned();
            let name = d.name().to_string_lossy().into_owned();
            let fs_type = d.file_system().to_string_lossy().into_owned();

            // Skip Time Machine and other system-managed snapshots.
            if mount.starts_with("/System/Volumes/VM")
                || mount.starts_with("/System/Volumes/Preboot")
                || mount.starts_with("/System/Volumes/Update")
                || mount.starts_with("/System/Volumes/xarts")
                || mount.starts_with("/System/Volumes/iSCPreboot")
                || mount.starts_with("/System/Volumes/Hardware")
                || mount.starts_with("/System/Volumes/Recovery")
                || mount == "/System/Volumes/Data"  // appears as "Macintosh HD - Data"
                || mount.starts_with("/private/var/vm")
            {
                return None;
            }

            let total = d.total_space();
            let free = d.available_space();
            if total == 0 {
                return None;
            }

            Some(DiskInfo {
                name,
                mount_point: mount.clone(),
                fs_type,
                total_bytes: total,
                used_bytes: total.saturating_sub(free),
                is_removable: d.is_removable(),
                is_boot: mount == "/",
            })
        })
        .collect();

    // Boot volume first, then by total size desc.
    out.sort_by(|a, b| {
        b.is_boot
            .cmp(&a.is_boot)
            .then_with(|| b.total_bytes.cmp(&a.total_bytes))
    });
    out
}
