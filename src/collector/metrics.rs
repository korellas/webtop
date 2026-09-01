use crate::collector::fan::FanReader;
use crate::collector::snapshot::{ProcessInfo, SystemSnapshot};
use crate::collector::{battery, gpu_procs, mem_breakdown};
use crate::storage::db::MetricsDb;
use chrono::{Datelike, Local};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use sysinfo::{
    CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, RefreshKind, System,
};

/// Meta keys used to persist small scalars across process restarts.
const META_ENERGY_WH: &str = "energy_session_wh";
const META_ENERGY_MONTH: &str = "energy_session_month"; // YYYY-MM
const META_ENERGY_PREV_MONTH_WH: &str = "energy_prev_month_wh";
/// Idempotency flag for the v1 reconciliation that heals the early
/// divergence between `meta.energy_session_wh` (which kept counting)
/// and `energy_daily` (which didn't exist yet). Once set, the
/// reconciliation routine never runs again.
const META_ENERGY_RECONCILED_V1: &str = "energy_reconciled_v1";

/// Rolling window (seconds) used to smooth per-process CPU / GPU numbers.
const PROCESS_WINDOW_SECS: f32 = 5.0;

#[derive(Clone)]
struct ProcessSample {
    at: Instant,
    cpu: f32,
    gpu: f32,
    mem: u64,
    name: String,
    user: String,
    cmd: String,
}

/// Collector ticks between disk-list refreshes.
///
/// The collector's real cadence is ~2 s, so 15 ticks is roughly half a minute.
/// The disk list only supplies the root volume's used/total — a number that
/// moves on the scale of downloads, not of ticks. Starting the counter *at*
/// this value means the first tick after start refreshes, so the very first
/// snapshot carries real capacity rather than zeros.
const DISK_REFRESH_TICKS: u32 = 15;

pub struct MetricsCollector {
    sys: System,
    disks: Disks,
    networks: Networks,
    prev_net_rx: u64,
    prev_net_tx: u64,
    energy_session_wh: f64,
    energy_prev_month_wh: f64,
    /// Stored as a YYYY-MM string so month detection is timezone-stable
    /// and we don't have to care about year boundaries.
    energy_month: String,
    last_tick: Option<Instant>,
    /// Ticks since the disk list was last refreshed. See `DISK_REFRESH_TICKS`.
    ticks_since_disk_refresh: u32,
    sampler: Option<macmon::Sampler>,
    /// Lazily-opened fan-RPM reader. None on fanless or when AppleSMC
    /// refuses to open. Lives next to `sampler` because both rely on
    /// IOKit and live for the collector's whole lifetime.
    fan_reader: Option<FanReader>,
    process_history: HashMap<u32, VecDeque<ProcessSample>>,
    prev_gpu_time_ns: HashMap<u32, u64>,
    prev_gpu_sample_at: Option<Instant>,
    /// Per-PID cumulative CPU seconds from the previous `ps` sample. Diffed
    /// against the current sample to derive instantaneous CPU% — the same
    /// "delta cputime / delta wallclock" method sysinfo uses internally,
    /// but applied to ps data so it works for every user's processes.
    prev_cpu_time: HashMap<u32, f64>,
    prev_proc_sample_at: Option<Instant>,
    /// DB handle for persisting counters that must survive restarts.
    /// Optional so unit tests can still construct a collector without a DB.
    db: Option<Arc<MetricsDb>>,
    /// Cached battery info — refreshed once every ~10s since it changes slowly
    /// and shelling out to `pmset`/`ioreg` is relatively expensive.
    cached_battery: Option<crate::collector::snapshot::BatteryInfo>,
    last_battery_read: Option<Instant>,
}

fn current_month_key() -> String {
    let now = Local::now();
    format!("{:04}-{:02}", now.year(), now.month())
}

impl MetricsCollector {
    /// Convenience constructor when there's no DB (tests, dry runs).
    pub fn new() -> Self {
        Self::with_db(None)
    }

    /// Whether the macmon sampler is available. When `true`, `collect()` paces
    /// itself at ~1 s per call via the sampler's internal sleep, so the
    /// collector loop does NOT need its own `thread::sleep`.
    pub fn sampler_is_active(&self) -> bool {
        self.sampler.is_some()
    }

    /// Construct a collector that will persist cross-restart scalars
    /// (notably `energy_session_wh`) into the provided DB.
    pub fn with_db(db: Option<Arc<MetricsDb>>) -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        sys.refresh_all();

        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();

        let (net_rx, net_tx) = Self::total_network(&networks);

        let sampler = macmon::Sampler::new().ok();

        let current_month = current_month_key();

        // Restore energy counter if the stored month matches the current
        // calendar month; otherwise start at zero for a fresh monthly total.
        let mut energy_session_wh = 0.0;
        let mut energy_prev_month_wh = 0.0;
        if let Some(ref db) = db {
            // Always restore the previous month total (it survives month boundaries).
            if let Some(raw) = db.meta_get(META_ENERGY_PREV_MONTH_WH) {
                if let Ok(wh) = raw.parse::<f64>() {
                    if wh.is_finite() && wh >= 0.0 {
                        energy_prev_month_wh = wh;
                    }
                }
            }

            let stored_month = db.meta_get(META_ENERGY_MONTH);
            if stored_month.as_deref() == Some(current_month.as_str()) {
                if let Some(raw) = db.meta_get(META_ENERGY_WH) {
                    if let Ok(wh) = raw.parse::<f64>() {
                        if wh.is_finite() && wh >= 0.0 {
                            energy_session_wh = wh;
                        }
                    }
                }
            } else {
                // New month (or first run) — persist the fresh baseline so a
                // subsequent restart picks up the correct month marker.
                db.meta_set(META_ENERGY_MONTH, &current_month);
                db.meta_set(META_ENERGY_WH, "0");
            }

            // Seed the durable per-day energy store from whatever days still
            // exist in `metrics_raw` so a fresh upgrade doesn't show empty
            // bars for last week. Today is skipped — live accumulation owns
            // it and we don't want to race with our own writes below.
            let today_key = Local::now().format("%Y-%m-%d").to_string();
            let _ = db.backfill_energy_daily(&today_key);
            // Same seeding for the network Day/Week/Month tabs — `insert_raw`
            // owns today's live accumulation into `network_daily`.
            let _ = db.backfill_network_daily(&today_key);

            // One-time reconciliation (v1): older versions of webtop
            // accumulated `meta.energy_session_wh` but predated the
            // `energy_daily` table, so the cumulative session card would
            // read e.g. 1.7 kWh while the day/week/month charts stayed
            // empty. On first boot after the upgrade, attribute the gap
            // to today's row so the two views agree. Guarded by a meta
            // flag so it NEVER runs twice.
            if db.meta_get(META_ENERGY_RECONCILED_V1).is_none() {
                if energy_session_wh > 0.0 {
                    let daily_sum_this_month = db.sum_energy_daily_for_month(&current_month);
                    let gap = energy_session_wh - daily_sum_this_month;
                    // Only reconcile if the gap is meaningful (>10 Wh).
                    // Smaller gaps are rounding noise and not worth
                    // distorting today's chart for.
                    if gap > 10.0 {
                        db.add_daily_energy(&today_key, gap);
                        eprintln!(
                            "energy: reconciled {gap:.1} Wh gap between \
                             session counter ({energy_session_wh:.1} Wh) \
                             and {current_month} daily sum ({daily_sum_this_month:.1} Wh)"
                        );
                    }
                }
                db.meta_set(META_ENERGY_RECONCILED_V1, "1");
            }
        }

        Self {
            sys,
            disks,
            networks,
            prev_net_rx: net_rx,
            prev_net_tx: net_tx,
            energy_session_wh,
            energy_prev_month_wh,
            energy_month: current_month,
            last_tick: None,
            ticks_since_disk_refresh: DISK_REFRESH_TICKS,
            sampler,
            // Eagerly open SMC for fan reads. Doing it once at startup
            // avoids opening/closing the IOKit handle on every tick.
            // None → fanless Mac or SMC unavailable; we surface 0.0 RPM.
            fan_reader: FanReader::new(),
            process_history: HashMap::new(),
            prev_gpu_time_ns: HashMap::new(),
            prev_gpu_sample_at: None,
            prev_cpu_time: HashMap::new(),
            prev_proc_sample_at: None,
            db,
            cached_battery: None,
            last_battery_read: None,
        }
    }

    pub fn collect(&mut self) -> SystemSnapshot {
        // Narrow, not `refresh_all()`. Everything this function reads out of
        // `sys` is here: global and per-core CPU usage, memory and swap, and
        // each process's disk-usage delta. `refresh_all()` additionally
        // re-read the command line, executable path, working directory,
        // environment and owning user of every process on the machine —
        // roughly 900 of them, every two seconds — and none of that is used.
        // Process *identity* comes from two `ps` passes (see
        // `collector::processes`) and per-process memory from
        // `collector::footprint`, neither of which goes through sysinfo.
        self.sys.refresh_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::everything())
                .with_processes(ProcessRefreshKind::nothing().with_disk_usage()),
        );

        // The disk list feeds exactly one number pair — the root volume's
        // used/total — and that is not a two-second quantity. Refreshing it
        // walks every mounted volume through CoreFoundation URL property
        // reads, which was the single largest non-sleeping item in the
        // collector's profile. Read/write *rates* are unaffected: they come
        // from per-process disk deltas above, not from here.
        self.ticks_since_disk_refresh += 1;
        if self.ticks_since_disk_refresh >= DISK_REFRESH_TICKS {
            self.ticks_since_disk_refresh = 0;
            self.disks.refresh(true);
        }

        self.networks.refresh(true);

        // CPU
        let cpu_total = self.sys.global_cpu_usage();
        let cpus = self.sys.cpus();
        let num_cpus = cpus.len();
        let cpu_cores: Vec<f32> = cpus.iter().map(|c| c.cpu_usage()).collect();
        let (mut cpu_p_cores, mut cpu_e_cores) = if num_cpus > 4 {
            let e_count = num_cpus / 2;
            let e_avg: f32 =
                cpus[..e_count].iter().map(|c| c.cpu_usage()).sum::<f32>() / e_count as f32;
            let p_avg: f32 = cpus[e_count..].iter().map(|c| c.cpu_usage()).sum::<f32>()
                / (num_cpus - e_count) as f32;
            (p_avg, e_avg)
        } else {
            (cpu_total, cpu_total)
        };

        // Memory
        let mem_used = self.sys.used_memory();
        let mem_total = self.sys.total_memory();
        let mem_swap_used = self.sys.used_swap();
        let mem_breakdown_val = mem_breakdown::collect();

        // Battery (refreshed every ~10s; shelling out is expensive)
        let now_inst = Instant::now();
        let battery_val = match self.last_battery_read {
            Some(at) if now_inst.duration_since(at) < Duration::from_secs(10) => {
                self.cached_battery.clone()
            }
            _ => {
                let b = battery::collect();
                self.cached_battery = b.clone();
                self.last_battery_read = Some(now_inst);
                b
            }
        };

        // Disk I/O — sysinfo's per-process `read_bytes`/`written_bytes` are
        // ALREADY the delta since the last refresh (the Apple backend subtracts
        // the previous cumulative internally), so total_disk_io returns THIS
        // tick's bytes directly. Unlike the cumulative network counters we must
        // NOT subtract a previous total — doing so was a delta-of-deltas that
        // collapsed to ~0 under steady I/O. Just rate-normalise below.
        let (disk_read_delta, disk_write_delta) = Self::total_disk_io(&self.sys);

        // Disk capacity
        let (disk_used, disk_total) = self.disk_capacity();

        // Network I/O — raw cumulative-counter deltas (rate computed below).
        let (net_rx, net_tx) = Self::total_network(&self.networks);
        let net_down_delta = net_rx.saturating_sub(self.prev_net_rx);
        let net_up_delta = net_tx.saturating_sub(self.prev_net_tx);
        self.prev_net_rx = net_rx;
        self.prev_net_tx = net_tx;

        // GPU + power metrics via macmon (Apple Silicon only; falls back to 0 gracefully).
        // Power split is CPU / GPU / Other, where Other = (system-wide) - CPU - GPU
        // and captures ANE, DRAM, display, network, SoC fabric, etc.
        let mut gpu_usage = 0.0f32;
        let mut power_total_w = 0.0f32;
        let mut power_cpu_w = 0.0f32;
        let mut power_gpu_w = 0.0f32;
        let mut power_other_w = 0.0f32;
        let mut cpu_temp_c = 0.0f32;
        // GPU die temperature is intentionally not collected: M3/M3 Ultra/M4
        // gate the `Tg*` SMC keys when the GPU is idle, which produces a
        // misleading "0 °C" most of the time. CPU die temp covers system
        // thermal state since both clusters share the same SoC die. See
        // https://github.com/vladkens/macmon/issues/12 for the upstream
        // discussion. Snapshot field is preserved (always 0) for wire-
        // format / DB-schema stability.
        let gpu_temp_c = 0.0f32;

        if let Some(ref mut sampler) = self.sampler {
            // Sample for ~1 s: `macmon::Sampler::get_metrics` averages the
            // IOReport CPU / GPU power counters across the window. Short
            // windows (e.g. 100 ms) are unreliable — individual deltas can
            // come back as all-CPU one tick and near-zero the next, which
            // makes the Power breakdown appear to oscillate between CPU
            // and "other" even though sys_power stays flat. The longer
            // window hands us a stable per-subsystem average.
            //
            // This call blocks for the full window, so it also doubles as
            // our tick pacing — see the collector thread below.
            if let Ok(m) = sampler.get_metrics(1000) {
                // macmon returns utilization as a 0-1 ratio — scale to 0-100%.
                gpu_usage = (m.gpu_usage.1 * 100.0).clamp(0.0, 100.0);
                power_cpu_w = m.cpu_power.max(0.0).min(500.0);
                power_gpu_w = m.gpu_power.max(0.0).min(500.0);

                // Prefer system-wide SMC reading (sys_power) when available —
                // it includes DRAM, display, Wi-Fi, SoC fabric, etc.
                // Fall back to cpu + gpu + ane when SMC is unreadable.
                let system_w = if m.sys_power > 0.0 {
                    m.sys_power
                } else {
                    m.cpu_power + m.gpu_power + m.ane_power
                };
                power_total_w = system_w.max(0.0).min(500.0);
                power_other_w = (power_total_w - power_cpu_w - power_gpu_w).max(0.0);

                // macmon pcpu/ecpu usage: use if non-zero, else fall back to sysinfo
                let p = (m.pcpu_usage.1 * 100.0).clamp(0.0, 100.0);
                let e = (m.ecpu_usage.1 * 100.0).clamp(0.0, 100.0);
                if p > 0.0 || e > 0.0 {
                    cpu_p_cores = p;
                    cpu_e_cores = e;
                }

                // CPU die temperature — macmon's mean of `Tp*`/`Te*`/`Ts*`
                // (and `TPD*` on M4) works reliably. 0.0 means
                // "unavailable" and we clamp absurd readings to keep the
                // y-axis sane.
                cpu_temp_c = sanitize_temp(m.temp.cpu_temp_avg);
            }
        }

        // Fan speed — SMC keys `FNum` + `F{i}Ac`. Cheap; skip silently if
        // the reader couldn't open (fanless Mac or SMC closed).
        let fan_rpm = self
            .fan_reader
            .as_mut()
            .map(|r| r.max_rpm())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(0.0);

        // Stamp the snapshot **after** the macmon 1-second IOReport window
        // closes — i.e. the moment the data is "crystallised". Stamping at
        // the start of collect would put the timestamp ~1.2 s in the past
        // by the time the WS subscriber receives the frame, leaving the
        // chart line ending well short of the right edge while the card
        // (which shows the same value) appears live. End-stamping aligns
        // the chart tail with the X-axis "now" so card and chart match.
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        // --- Energy accumulation (month-scoped, persisted across restarts) ---
        let current_month = current_month_key();
        if current_month != self.energy_month {
            // Month rollover — save the just-finished month's total, then
            // reset the cumulative counter.
            self.energy_prev_month_wh = self.energy_session_wh;
            self.energy_session_wh = 0.0;
            self.energy_month = current_month.clone();
            if let Some(ref db) = self.db {
                db.meta_set(
                    META_ENERGY_PREV_MONTH_WH,
                    &self.energy_prev_month_wh.to_string(),
                );
                db.meta_set(META_ENERGY_MONTH, &self.energy_month);
                db.meta_set(META_ENERGY_WH, "0");
            }
        }

        // Real elapsed time since the previous tick — the single denominator
        // for every "per second" figure. None on the very first tick (no prior
        // reference). Measured at the same point each loop, so it equals the
        // loop period (~1.8 s), which is also the span the byte/energy deltas
        // above accrued over.
        let now = Instant::now();
        let tick_dt = self
            .last_tick
            .map(|prev| now.duration_since(prev).as_secs_f64());

        // Byte rates: divide the cumulative-counter deltas by the *unclamped*
        // elapsed time. Skipping this (treating a delta as bytes/sec) inflated
        // every rate by the tick interval — a 500 Mbps link read as ~900 Mbps.
        // Unclamped so a post-sleep sample reports its true low average rather
        // than a giant one-off spike.
        let disk_read_bytes_sec = per_sec_rate(disk_read_delta, tick_dt);
        let disk_write_bytes_sec = per_sec_rate(disk_write_delta, tick_dt);
        let net_down_bytes_sec = per_sec_rate(net_down_delta, tick_dt);
        let net_up_bytes_sec = per_sec_rate(net_up_delta, tick_dt);

        // Energy uses the same delta but clamped to [0, 5] s so a sleep /
        // clock-skew gap can't book a giant lump of Wh.
        let mut tick_delta_wh = 0.0f64;
        if let Some(dt) = tick_dt {
            let dt_secs = dt.clamp(0.0, 5.0);
            tick_delta_wh = (power_total_w as f64) * dt_secs / 3600.0;
            self.energy_session_wh += tick_delta_wh;
        }
        self.last_tick = Some(now);

        // Persist the running counters every tick. SQLite WAL handles this
        // trivially (~1 write/sec). On crash or kill, at most the last
        // second of accumulation is lost. We persist TWO counters:
        //   • `energy_session_wh` in `meta` — cumulative month-to-date,
        //     the number shown in the "this session" summary card.
        //   • A per-local-day row in `energy_daily` — the source of the
        //     day/week/month bar charts. Kept in sync with the session
        //     counter by using the same `tick_delta_wh` both increment.
        if let Some(ref db) = self.db {
            db.meta_set(META_ENERGY_WH, &self.energy_session_wh.to_string());
            if tick_delta_wh > 0.0 {
                let day_key = Local::now().format("%Y-%m-%d").to_string();
                db.add_daily_energy(&day_key, tick_delta_wh);
            }
        }

        // --- Processes: 5-second rolling window average ---------------------
        //
        // Per-process GPU usage is sampled from `AGXDeviceUserClient`
        // entries in the I/O Kit registry (cumulative accumulatedGPUTime
        // in nanoseconds, per PID). Diffing against the previous sample
        // and dividing by wall-clock elapsed gives instantaneous GPU
        // utilisation — the same method `mactop` uses, no root required.
        let current_gpu_ns = gpu_procs::sample();
        let mut per_pid_gpu_pct: HashMap<u32, f32> = HashMap::new();
        if let Some(prev_at) = self.prev_gpu_sample_at {
            let elapsed = now.duration_since(prev_at).as_secs_f64().max(0.05);
            let mut raw_total_pct: f64 = 0.0;
            for (pid, cur_ns) in &current_gpu_ns {
                let prev_ns = self.prev_gpu_time_ns.get(pid).copied().unwrap_or(0);
                if *cur_ns >= prev_ns {
                    let delta_ns = (*cur_ns - prev_ns) as f64;
                    // ns of GPU work / wall-ns of elapsed time = GPU fraction
                    // → multiply by 100 for a percent.
                    let pct = (delta_ns / (elapsed * 1_000_000_000.0)) * 100.0;
                    if pct > 0.0 {
                        per_pid_gpu_pct.insert(*pid, pct as f32);
                        raw_total_pct += pct;
                    }
                }
            }
            // Rescale so the per-process percentages sum to the authoritative
            // system-wide GPU% reading from macmon. Avoids both under- and
            // over-reporting when multiple command queues run concurrently.
            if raw_total_pct > 0.01 && (gpu_usage as f64) > 0.01 {
                let scale = (gpu_usage as f64) / raw_total_pct;
                for v in per_pid_gpu_pct.values_mut() {
                    *v = (*v as f64 * scale) as f32;
                }
            }
        }
        self.prev_gpu_time_ns = current_gpu_ns;
        self.prev_gpu_sample_at = Some(now);

        let total_cores = num_cpus as f32;

        // Enumerate ALL users' processes via `ps` (setuid-root). sysinfo
        // running as an unprivileged LaunchAgent reports 0 CPU/mem for
        // processes it doesn't own, so they'd silently drop off the list;
        // ps sees everything. Instantaneous CPU% is derived from the
        // cumulative CPU-time delta since the previous tick.
        let ps_rows = crate::collector::processes::sample();
        let cpu_dt = self
            .prev_proc_sample_at
            .map(|at| now.duration_since(at).as_secs_f64().max(0.05));
        let mut new_cpu_time: HashMap<u32, f64> = HashMap::with_capacity(ps_rows.len());

        let live_pids: Vec<(u32, f32, u64, String, String, String)> = ps_rows
            .iter()
            .map(|r| {
                new_cpu_time.insert(r.pid, r.cpu_time_secs);
                // per-core %: a process pinning one full core reads ~100.
                let cpu_per_core = match (cpu_dt, self.prev_cpu_time.get(&r.pid)) {
                    (Some(dt), Some(&prev)) if r.cpu_time_secs >= prev => {
                        ((r.cpu_time_secs - prev) / dt * 100.0) as f32
                    }
                    _ => 0.0,
                };
                // Normalise to system % (100% = all cores) to match the
                // convention the rest of the UI already uses.
                let cpu = (cpu_per_core / total_cores).clamp(0.0, 100.0);
                (
                    r.pid,
                    cpu,
                    r.mem_bytes,
                    r.name.clone(),
                    r.user.clone(),
                    r.args.clone(),
                )
            })
            .collect();
        self.prev_cpu_time = new_cpu_time;
        self.prev_proc_sample_at = Some(now);

        let live_pid_set: std::collections::HashSet<u32> =
            live_pids.iter().map(|(p, _, _, _, _, _)| *p).collect();

        for (pid, cpu, mem, name, user, cmd) in &live_pids {
            let gpu_pct = per_pid_gpu_pct.get(pid).copied().unwrap_or(0.0);
            let sample = ProcessSample {
                at: now,
                cpu: *cpu,
                gpu: gpu_pct,
                mem: *mem,
                name: name.clone(),
                user: user.clone(),
                cmd: cmd.clone(),
            };
            let q = self.process_history.entry(*pid).or_default();
            q.push_back(sample);
            while let Some(front) = q.front() {
                if now.duration_since(front.at).as_secs_f32() > PROCESS_WINDOW_SECS {
                    q.pop_front();
                } else {
                    break;
                }
            }
        }
        self.process_history
            .retain(|pid, _| live_pid_set.contains(pid));

        // Build averaged snapshot for every live PID, then emit the union of
        // (top 60 by CPU) ∪ (top 40 by memory) ∪ (top 40 by GPU) so heavy
        // GPU or memory users are still visible regardless of CPU share.
        let all_pids: Vec<ProcessInfo> = self
            .process_history
            .iter()
            .filter_map(|(pid, samples)| {
                if samples.is_empty() {
                    return None;
                }
                let n = samples.len() as f32;
                let cpu_avg = samples.iter().map(|s| s.cpu).sum::<f32>() / n;
                let gpu_avg = samples.iter().map(|s| s.gpu).sum::<f32>() / n;
                let last = samples.back().unwrap();
                Some(ProcessInfo {
                    pid: *pid,
                    name: last.name.clone(),
                    user: last.user.clone(),
                    cpu_percent: cpu_avg,
                    gpu_percent: gpu_avg,
                    mem_bytes: last.mem,
                    cmd: last.cmd.clone(),
                })
            })
            .collect();

        let mut by_cpu = all_pids.clone();
        by_cpu.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());
        by_cpu.truncate(60);

        let mut by_mem = all_pids.clone();
        by_mem.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes));
        by_mem.truncate(40);

        let mut by_gpu = all_pids;
        by_gpu.sort_by(|a, b| b.gpu_percent.partial_cmp(&a.gpu_percent).unwrap());
        by_gpu.truncate(40);

        let mut seen = std::collections::HashSet::new();
        let mut processes: Vec<ProcessInfo> = Vec::with_capacity(120);
        for p in by_cpu
            .into_iter()
            .chain(by_gpu.into_iter())
            .chain(by_mem.into_iter())
        {
            if seen.insert(p.pid) {
                processes.push(p);
            }
        }
        // Final default sort: CPU desc. Frontend can re-sort per column.
        processes.sort_by(|a, b| b.cpu_percent.partial_cmp(&a.cpu_percent).unwrap());

        SystemSnapshot {
            timestamp,
            cpu_total,
            cpu_p_cores,
            cpu_e_cores,
            cpu_cores,
            gpu_usage,
            mem_used,
            mem_total,
            mem_swap_used,
            mem_breakdown: mem_breakdown_val,
            disk_read_bytes_sec,
            disk_write_bytes_sec,
            disk_used,
            disk_total,
            net_up_bytes_sec,
            net_down_bytes_sec,
            power_total_w,
            power_cpu_w,
            power_gpu_w,
            power_other_w,
            cpu_temp_c,
            gpu_temp_c,
            fan_rpm,
            energy_session_wh: self.energy_session_wh,
            energy_prev_month_wh: self.energy_prev_month_wh,
            battery: battery_val,
            processes,
            // A live sample is a single instant — it has no range to summarise.
            // The chart falls back to the point value so the band collapses
            // onto the line across the live tail.
            band: None,
        }
    }

    fn total_network(networks: &Networks) -> (u64, u64) {
        let mut rx = 0u64;
        let mut tx = 0u64;
        for (name, data) in networks {
            if !is_countable_iface(name) {
                continue;
            }
            rx += data.total_received();
            tx += data.total_transmitted();
        }
        (rx, tx)
    }

    /// Sum of per-process disk bytes **since the last `refresh`** — i.e. this
    /// tick's read/write totals, NOT cumulative. `DiskUsage::read_bytes` is
    /// already `current - previous` inside sysinfo (see `total_read_bytes` for
    /// the lifetime figure), so callers must treat the return as a per-tick
    /// delta and only divide by elapsed time, never subtract a prior value.
    fn total_disk_io(sys: &System) -> (u64, u64) {
        let mut read = 0u64;
        let mut write = 0u64;
        for (_pid, proc_) in sys.processes() {
            let du = proc_.disk_usage();
            read += du.read_bytes;
            write += du.written_bytes;
        }
        (read, write)
    }

    fn disk_capacity(&self) -> (u64, u64) {
        // Prefer the root mount point (OS disk)
        for disk in self.disks.list() {
            if disk.mount_point() == std::path::Path::new("/") {
                let t = disk.total_space();
                let a = disk.available_space();
                return (t - a, t);
            }
        }
        // Fallback: largest disk
        let mut used = 0u64;
        let mut total = 0u64;
        for disk in self.disks.list() {
            let t = disk.total_space();
            let a = disk.available_space();
            if t > total {
                total = t;
                used = t - a;
            }
        }
        (used, total)
    }
}

/// Whether this interface should be included in the **aggregate** rx/tx
/// totals shown in the header cards and stored in history.
///
/// Reasoning: on macOS, the kernel exposes 20+ interfaces per box. Summing
/// *all* of them double-counts traffic that hops across layers. Concretely:
///
///   • VPN active → plaintext flows on `utun*`, ciphertext on `en0`. Summing
///     both inflates the total by ~2× relative to what the router sees.
///   • `bridge0` mirrors Thunderbolt / internet-sharing traffic that is
///     *already* counted on the physical interface it bridges.
///   • `awdl0`, `llw0`, `ap1` are peer-to-peer radios used by AirDrop /
///     Continuity; their counters are mostly noise.
///   • `anpi*`, `pktap*`, `gif*`, `stf*` are kernel pseudo-interfaces.
///   • `lo0` is loopback (localhost traffic).
///
/// By keeping only `en*`/`eth*` (the real NICs — Wi-Fi and Ethernet show up
/// as `en0`/`en1`/…) the webtop total matches the router's WAN-side reading
/// within a few percent, which is what users intuitively expect when they
/// compare the two.
fn is_countable_iface(name: &str) -> bool {
    if name.starts_with("lo")
        || name.starts_with("utun")
        || name.starts_with("ipsec")
        || name.starts_with("ppp")
        || name.starts_with("awdl")
        || name.starts_with("llw")
        || name.starts_with("ap")    // e.g. `ap1` — hotspot assist radio
        || name.starts_with("bridge")
        || name.starts_with("anpi")
        || name.starts_with("pktap")
        || name.starts_with("gif")
        || name.starts_with("stf")
        || name.starts_with("tap")
        || name.starts_with("tun")
    {
        return false;
    }
    // Whitelist the physical NIC family. Anything else (including one-off
    // vendor names) is excluded to stay conservative.
    name.starts_with("en") || name.starts_with("eth")
}

/// Convert a between-tick cumulative-counter delta into a per-second rate
/// using the ACTUAL elapsed time. Returns 0 when the interval is unknown
/// (first tick) or non-positive — never divides by zero, and never assumes a
/// 1 s cadence. Real ticks are ~1.8 s, so assuming 1 Hz would inflate every
/// byte rate ~1.8x (a 500 Mbps link reading as ~900 Mbps).
fn per_sec_rate(delta: u64, dt_secs: Option<f64>) -> u64 {
    match dt_secs {
        Some(s) if s > 0.0 => (delta as f64 / s).round() as u64,
        _ => 0,
    }
}

/// Sanitize a die-temperature reading from macmon.
///
/// macmon computes its averages with `zero_div(sum, count)` which returns
/// 0.0 when `count == 0`, so an empty sensor list reads as 0.0 not NaN —
/// but we still defend against NaN/Inf in case a future macmon revision
/// changes that, and against absurd readings caused by a flaky SMC key
/// (we've seen `flt ` keys briefly return 1e9 °C during boot).
fn sanitize_temp(c: f32) -> f32 {
    if !c.is_finite() {
        return 0.0;
    }
    c.clamp(0.0, 150.0)
}

#[cfg(test)]
mod tests {
    use super::{is_countable_iface, per_sec_rate};

    #[test]
    fn rate_divides_delta_by_real_interval() {
        // 100 MiB transferred during a 1.8 s tick is ~55.6 MiB/s, NOT 100 MiB/s.
        let delta = 100 * 1024 * 1024;
        assert_eq!(
            per_sec_rate(delta, Some(1.8)),
            (delta as f64 / 1.8).round() as u64
        );
        // An exact 1 s interval leaves the delta unchanged.
        assert_eq!(per_sec_rate(1_000_000, Some(1.0)), 1_000_000);
    }

    #[test]
    fn rate_is_zero_when_interval_unknown_or_nonpositive() {
        // First tick (no previous) and degenerate intervals must not divide by
        // zero or fabricate a rate.
        assert_eq!(per_sec_rate(123_456, None), 0);
        assert_eq!(per_sec_rate(123_456, Some(0.0)), 0);
        assert_eq!(per_sec_rate(123_456, Some(-1.0)), 0);
    }

    #[test]
    fn counts_physical_nics() {
        assert!(is_countable_iface("en0"));
        assert!(is_countable_iface("en1"));
        assert!(is_countable_iface("eth0"));
    }

    #[test]
    fn skips_virtual_and_peer_interfaces() {
        for n in [
            "lo0",
            "utun0",
            "utun10",
            "ipsec0",
            "ppp0",
            "awdl0",
            "llw0",
            "ap1",
            "bridge0",
            "bridge100",
            "anpi0",
            "pktap0",
            "gif0",
            "stf0",
            "tap0",
            "tun0",
        ] {
            assert!(!is_countable_iface(n), "{n} should be skipped");
        }
    }
}
