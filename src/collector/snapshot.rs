use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemSnapshot {
    pub timestamp: u64,
    pub cpu_total: f32,
    pub cpu_p_cores: f32,
    pub cpu_e_cores: f32,
    /// Per-core utilisation 0..100. Order matches `SystemInfo.core_kinds`.
    /// Empty on older aggregated history rows (which no longer store per-core data).
    #[serde(default)]
    pub cpu_cores: Vec<f32>,
    pub gpu_usage: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub mem_swap_used: u64,
    /// Detailed VM breakdown; zeroed when the Mach call fails.
    #[serde(default)]
    pub mem_breakdown: MemBreakdown,
    pub disk_read_bytes_sec: u64,
    pub disk_write_bytes_sec: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    pub net_up_bytes_sec: u64,
    pub net_down_bytes_sec: u64,
    pub power_total_w: f32,
    pub power_cpu_w: f32,
    pub power_gpu_w: f32,
    pub power_other_w: f32,
    /// CPU die temperature in Celsius. Apple Silicon only — 0.0 on Intel
    /// Macs or when the SMC/HID read fails.
    #[serde(default)]
    pub cpu_temp_c: f32,
    /// GPU die temperature in Celsius. Same caveat as `cpu_temp_c`.
    #[serde(default)]
    pub gpu_temp_c: f32,
    /// Highest fan RPM across all fans. 0.0 on fanless Macs (e.g.
    /// MacBook Air) or when the SMC `F0Ac` family of keys can't be read.
    #[serde(default)]
    pub fan_rpm: f32,
    pub energy_session_wh: f64,
    pub energy_prev_month_wh: f64,
    /// Battery info — None on desktop Macs or if IOKit reads fail.
    #[serde(default)]
    pub battery: Option<BatteryInfo>,
    pub processes: Vec<ProcessInfo>,
    /// Per-bucket min/max for the charted series — set only on aggregated
    /// history rows. A live 1 s sample *is* the extreme of its own bucket, so
    /// there is nothing to summarise; this stays `None` and is omitted from the
    /// WebSocket frame entirely rather than padding every push with zeros.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub band: Option<MetricBand>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    /// Owning user (e.g. "alice", "root", "_windowserver"). Sourced from
    /// `ps`, which — being setuid-root — can attribute every process even
    /// though webtop itself runs unprivileged. Defaulted for wire/history
    /// compatibility with rows that predate the field.
    #[serde(default)]
    pub user: String,
    pub cpu_percent: f32,
    pub gpu_percent: f32,
    pub mem_bytes: u64,
    /// Full command line, truncated. Distinguishes processes that share an
    /// executable name — four `python3.1` rows are indistinguishable until you
    /// can see which port and model each one was started with. Defaulted for
    /// wire/history compatibility with rows that predate the field.
    #[serde(default)]
    pub cmd: String,
}

/// Memory breakdown in bytes. `wired + active + inactive + compressed + free`
/// should approximately equal `mem_total` from the enclosing snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemBreakdown {
    pub wired: u64,
    pub active: u64,
    pub inactive: u64,
    pub compressed: u64,
    pub free: u64,
}

/// Battery state for laptops. `None` values mean the info wasn't readable.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BatteryInfo {
    /// Current charge 0..100.
    pub percent: f32,
    pub is_charging: bool,
    pub is_plugged_in: bool,
    pub time_remaining_sec: Option<u32>,
    pub cycle_count: Option<u32>,
    /// Battery health as a percentage of original design capacity.
    pub health_percent: Option<f32>,
    /// Instantaneous power flow — positive while charging, negative while discharging.
    pub charge_rate_w: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregatedMetric {
    pub timestamp: u64,
    pub cpu_total: f32,
    pub cpu_p_cores: f32,
    pub cpu_e_cores: f32,
    pub gpu_usage: f32,
    pub mem_used: u64,
    pub mem_swap_used: u64,
    pub disk_read_bytes_sec: u64,
    pub disk_write_bytes_sec: u64,
    pub net_up_bytes_sec: u64,
    pub net_down_bytes_sec: u64,
    pub power_total_w: f32,
    pub power_cpu_w: f32,
    pub power_gpu_w: f32,
    pub power_other_w: f32,
    #[serde(default)]
    pub cpu_temp_c: f32,
    #[serde(default)]
    pub gpu_temp_c: f32,
    #[serde(default)]
    pub fan_rpm: f32,
    /// Per-bucket extremes for the series the dashboard actually charts.
    ///
    /// The averages above answer "what was typical"; alone they actively
    /// mislead at long timescales. A 2-second 100 % CPU spike inside a 4-minute
    /// 24 h bucket averages down to well under 1 % — the saturation that was
    /// plainly visible at 5 m becomes invisible at 24 h. Carrying min/max lets
    /// the chart draw the full range as a band behind the mean line, which is
    /// the standard treatment in time-series dashboards for exactly this reason.
    pub band: MetricBand,
}

/// Min/max pairs, `[min, max]`, for the charted series.
///
/// Serialises as two-element arrays, which keeps the history payload roughly
/// 2× rather than the ~3× that flat `_min`/`_max` field pairs would cost.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricBand {
    pub cpu_total: [f32; 2],
    pub gpu_usage: [f32; 2],
    pub mem_used: [u64; 2],
    pub power_total_w: [f32; 2],
    pub net_up_bytes_sec: [u64; 2],
    pub net_down_bytes_sec: [u64; 2],
    pub disk_read_bytes_sec: [u64; 2],
    pub disk_write_bytes_sec: [u64; 2],
    pub cpu_temp_c: [f32; 2],
}
