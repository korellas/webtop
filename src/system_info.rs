use serde::Serialize;
use std::process::Command;
use sysinfo::System;

#[derive(Debug, Clone, Serialize)]
pub struct SystemInfo {
    pub model: String,
    pub chip: String,
    pub p_core_count: u32,
    pub e_core_count: u32,
    pub gpu_core_count: u32,
    pub mem_total: u64,
    pub disk_total: u64,
    pub os_version: String,
    /// Detected active link speed in bytes/sec. Defaults to 125_000_000 (1 Gbps)
    /// when the active interface speed cannot be determined (e.g., Wi-Fi).
    pub net_link_speed_bytes_sec: u64,
    /// Per-logical-core kind, same length and order as `SystemSnapshot.cpu_cores`.
    /// `"P"` or `"E"`.
    pub core_kinds: Vec<String>,
}

impl SystemInfo {
    pub fn gather() -> Self {
        let sys = System::new_all();

        let model = sysctl_string("hw.model").unwrap_or_else(|| "Mac".into());
        let chip = System::cpu_arch();

        let p_core_count = sysctl_u32("hw.perflevel0.physicalcpu").unwrap_or(4);
        let e_core_count = sysctl_u32("hw.perflevel1.physicalcpu").unwrap_or(4);
        let gpu_core_count = sysctl_u32("hw.perflevel0.gpucorecount")
            .or_else(|| gpu_core_count_from_profiler())
            .unwrap_or(0);

        // Logical core counts (SMT normally off on Apple Silicon, so logical == physical,
        // but be defensive for Intel Macs.)
        let p_logical = sysctl_u32("hw.perflevel0.logicalcpu").unwrap_or(p_core_count);
        let e_logical = sysctl_u32("hw.perflevel1.logicalcpu").unwrap_or(e_core_count);

        // sysinfo orders CPUs with E-cores first, then P-cores (observed on Apple Silicon).
        // Encode that order here so the frontend can group bars correctly without having
        // to re-derive from `cpu_usage` values.
        let mut core_kinds: Vec<String> = Vec::with_capacity((e_logical + p_logical) as usize);
        for _ in 0..e_logical {
            core_kinds.push("E".into());
        }
        for _ in 0..p_logical {
            core_kinds.push("P".into());
        }
        // If the observed CPU count disagrees, fall back to marking everything as "P".
        let observed = sys.cpus().len() as u32;
        if observed != e_logical + p_logical {
            core_kinds = (0..observed).map(|_| "P".to_string()).collect();
        }

        let mem_total = sys.total_memory();

        let disks = sysinfo::Disks::new_with_refreshed_list();
        let disk_total = disks
            .list()
            .iter()
            .find(|d| d.mount_point() == std::path::Path::new("/"))
            .map(|d| d.total_space())
            .unwrap_or_else(|| {
                disks
                    .list()
                    .iter()
                    .map(|d| d.total_space())
                    .max()
                    .unwrap_or(0)
            });

        let os_version = System::long_os_version().unwrap_or_default();
        let net_link_speed_bytes_sec = detect_link_speed();

        Self {
            model,
            chip,
            p_core_count,
            e_core_count,
            gpu_core_count,
            mem_total,
            disk_total,
            os_version,
            net_link_speed_bytes_sec,
            core_kinds,
        }
    }
}

/// Detect the fastest active Ethernet link speed by parsing `ifconfig` output.
/// Falls back to 125_000_000 bytes/sec (1 Gbps) when no wired link is found
/// (e.g., when the Mac is on Wi-Fi only).
fn detect_link_speed() -> u64 {
    const DEFAULT: u64 = 125_000_000; // 1 Gbps in bytes/sec

    let output = Command::new("sh")
        .args([
            "-c",
            "ifconfig | grep -A20 'status: active' | grep 'media:'",
        ])
        .output();

    let Ok(out) = output else {
        return DEFAULT;
    };
    if !out.status.success() {
        return DEFAULT;
    }

    let text = String::from_utf8_lossy(&out.stdout);
    let mut max_bps: u64 = 0;
    for line in text.lines() {
        let l = line.to_ascii_lowercase();
        // Match common Ethernet media strings e.g. "1000baseT", "10Gbase-T", "100baseTX"
        if l.contains("10gbase") || l.contains("10000base") {
            max_bps = max_bps.max(10_000_000_000 / 8);
        } else if l.contains("1000base") || l.contains("1gbase") {
            max_bps = max_bps.max(1_000_000_000 / 8);
        } else if l.contains("100base") {
            max_bps = max_bps.max(100_000_000 / 8);
        }
    }

    if max_bps > 0 {
        max_bps
    } else {
        DEFAULT
    }
}

fn sysctl_string(key: &str) -> Option<String> {
    let output = Command::new("sysctl").arg("-n").arg(key).output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

fn sysctl_u32(key: &str) -> Option<u32> {
    sysctl_string(key)?.parse().ok()
}

fn gpu_core_count_from_profiler() -> Option<u32> {
    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let displays = json.get("SPDisplaysDataType")?.as_array()?;
    for display in displays {
        if let Some(cores) = display.get("sppci_cores") {
            if let Some(s) = cores.as_str() {
                // Format is like "38_gpu_cores" or just a number
                let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
                return digits.parse().ok();
            }
        }
    }
    None
}
