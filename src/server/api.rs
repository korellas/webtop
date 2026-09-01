use crate::collector::snapshot::SystemSnapshot;
use crate::server::AppState;
use crate::storage::db::MetricsDb;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub range: Option<String>,
}

/// Window duration for a `range` query param. Shared by `/api/history` and
/// `/api/network_totals` so the two can never disagree about what "1h" means.
fn window_ms_for_range(range: &str) -> Option<u64> {
    Some(match range {
        "1m" => 60_000,
        "5m" => 300_000,
        "15m" => 900_000,
        "1h" => 3_600_000,
        "24h" => 86_400_000,
        "7d" => 7 * 86_400_000,
        _ => return None,
    })
}

pub async fn get_system_info(
    State(state): State<Arc<AppState>>,
) -> Json<crate::system_info::SystemInfo> {
    Json(state.system_info.clone())
}

pub async fn get_history(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HistoryQuery>,
) -> Result<Json<Vec<SystemSnapshot>>, StatusCode> {
    let range = params.range.as_deref().unwrap_or("1h");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // (window_ms, bucket_ms)
    //
    // Bucket sizes match the frontend's render resolution (`MAX_POINTS` in
    // `chart-utils.ts`). If the backend were coarser than the frontend wants,
    // a page refresh would show a sparser chart than the live-running one
    // — because live WebSocket samples fill in the finer grid, while
    // historical rows only cover one slot per minute.
    // Bucket widths are sized against the collector's REAL cadence, which is
    // ~2 s, not the 1 s the code comments long claimed. `collect()` blocks ~1 s
    // inside `macmon::Sampler::get_metrics(1000)` and spends roughly another
    // second refreshing sysinfo, enumerating processes and reading disk/network
    // counters. Measured 2026-08-01: 29 rows in 60 s, inter-row gaps 1.99–2.16 s.
    //
    // This matters because a bucket narrower than ~2× the sample period holds a
    // single row, so `AVG`/`MIN`/`MAX` all collapse to that one value: no
    // aggregation happens, the chart draws raw sample-to-sample noise, and the
    // min/max band is zero-width. The 5 m view was the visible symptom — 2 s
    // buckets against 2 s samples, so a "5 minute overview" was really an
    // unsmoothed trace of every individual reading.
    //
    // 1 m deliberately stays at one sample per bucket: at that range you want
    // every reading, and there are only ~30 of them in the window anyway.
    // `MAX_POINTS` in `chart-utils.ts` mirrors these — keep the two in sync or
    // the client re-buckets on a different grid than the server used.
    let window_ms = window_ms_for_range(range).ok_or(StatusCode::BAD_REQUEST)?;
    let bucket_ms: u64 = match range {
        "1m" => 2_000,    // 30 pts — raw, 1 sample/bucket by design
        "5m" => 4_000,    // 75 pts, ~2 samples/bucket
        "15m" => 6_000,   // 150 pts, ~3 samples/bucket
        "1h" => 10_000,   // 360 pts, ~5 samples/bucket
        "24h" => 240_000, // 360 pts, ~116 samples/bucket
        // 28 min, not 30: a whole multiple of the rollup's 4-minute grid, so
        // this range is served from pre-aggregated buckets. See
        // `storage::rollup`. 360 pts, ~840 samples/bucket.
        "7d" => 1_680_000,
        _ => unreachable!("validated by window_ms_for_range above"),
    };

    let since = now.saturating_sub(window_ms);
    // Long ranges read pre-aggregated buckets; short ones go straight to the
    // raw rows, where the window is small enough that folding it costs less
    // than a millisecond. `rollup_serves` is the only thing that decides, so a
    // future bucket width that does not divide the rollup grid degrades to the
    // raw path rather than returning something subtly wrong.
    let rows = if MetricsDb::rollup_serves(bucket_ms) {
        state.db.query_bucketed_rolled(since, bucket_ms)
    } else {
        state.db.query_bucketed(since, bucket_ms)
    }
    .unwrap_or_default();

    Ok(Json(
        rows.into_iter()
            .map(|m| SystemSnapshot {
                timestamp: m.timestamp,
                cpu_total: m.cpu_total,
                cpu_p_cores: m.cpu_p_cores,
                cpu_e_cores: m.cpu_e_cores,
                cpu_cores: Vec::new(),
                gpu_usage: m.gpu_usage,
                mem_used: m.mem_used,
                mem_total: 0,
                mem_swap_used: m.mem_swap_used,
                mem_breakdown: Default::default(),
                disk_read_bytes_sec: m.disk_read_bytes_sec,
                disk_write_bytes_sec: m.disk_write_bytes_sec,
                disk_used: 0,
                disk_total: 0,
                net_up_bytes_sec: m.net_up_bytes_sec,
                net_down_bytes_sec: m.net_down_bytes_sec,
                power_total_w: m.power_total_w,
                power_cpu_w: m.power_cpu_w,
                power_gpu_w: m.power_gpu_w,
                power_other_w: m.power_other_w,
                cpu_temp_c: m.cpu_temp_c,
                gpu_temp_c: m.gpu_temp_c,
                fan_rpm: m.fan_rpm,
                energy_session_wh: 0.0,
                energy_prev_month_wh: 0.0,
                battery: None,
                processes: vec![],
                band: Some(m.band),
            })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct NetworkTotals {
    pub up_bytes: f64,
    pub down_bytes: f64,
}

/// Total bytes transferred inside `range`, trapezoid-integrated from raw
/// per-sample rates. See `MetricsDb::net_totals` for why this doesn't just
/// multiply `/api/history`'s bucket averages by the bucket width.
pub async fn get_network_totals(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HistoryQuery>,
) -> Result<Json<NetworkTotals>, StatusCode> {
    let range = params.range.as_deref().unwrap_or("1h");
    let window_ms = window_ms_for_range(range).ok_or(StatusCode::BAD_REQUEST)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let since = now.saturating_sub(window_ms);
    let (up_bytes, down_bytes) = state.db.net_totals(since).unwrap_or((0.0, 0.0));
    Ok(Json(NetworkTotals {
        up_bytes,
        down_bytes,
    }))
}

pub async fn get_processes(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<crate::collector::snapshot::ProcessInfo>> {
    let rb = crate::sync::guard(state.ring_buffer.read());
    match rb.latest() {
        Some(snap) => Json(snap.processes.clone()),
        None => Json(vec![]),
    }
}

// ─── New detail-drawer endpoints ────────────────────────────────────────────

pub async fn get_disks() -> Json<Vec<crate::collector::disks::DiskInfo>> {
    Json(crate::collector::disks::list_disks())
}

pub async fn get_network_interfaces(
) -> Json<Vec<crate::collector::net_interfaces::NetInterfaceInfo>> {
    Json(crate::collector::net_interfaces::list_interfaces())
}

/// Top GPU-using processes — derived from the latest snapshot's process list.
pub async fn get_gpu_processes(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<crate::collector::snapshot::ProcessInfo>> {
    let rb = crate::sync::guard(state.ring_buffer.read());
    let mut procs: Vec<_> = match rb.latest() {
        Some(snap) => snap
            .processes
            .iter()
            .filter(|p| p.gpu_percent > 0.05)
            .cloned()
            .collect(),
        None => vec![],
    };
    // `partial_cmp` is None for NaN, which a bad GPU sample can produce —
    // unwrapping here would panic the handler. Treat those as equal.
    procs.sort_by(|a, b| {
        b.gpu_percent
            .partial_cmp(&a.gpu_percent)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    procs.truncate(8);
    Json(procs)
}

#[derive(Deserialize)]
pub struct EnergyHistoryQuery {
    pub group: Option<String>,
    /// Browser's `Date.getTimezoneOffset()` — minutes the local TZ is BEHIND
    /// UTC (so KST = UTC+9 passes -540). Missing → server defaults to local.
    pub tz_offset_minutes: Option<i32>,
}

pub async fn get_energy_history(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EnergyHistoryQuery>,
) -> Result<Json<crate::storage::db::EnergyHistory>, StatusCode> {
    let group = params.group.as_deref().unwrap_or("hour");
    // Fall back to the server's local-offset if the client didn't send one.
    // `chrono::Local::now().offset()` returns seconds-east-of-UTC, which is
    // the negated form of JS's minutes-behind-UTC.
    let tz = params.tz_offset_minutes.unwrap_or_else(|| {
        let secs_east = chrono::Local::now().offset().local_minus_utc();
        -(secs_east / 60)
    });
    match state.db.energy_history(group, tz) {
        Ok(Some(h)) => Ok(Json(h)),
        Ok(None) => Err(StatusCode::BAD_REQUEST),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize)]
pub struct NetworkHistoryQuery {
    pub group: Option<String>,
    /// Same contract as `EnergyHistoryQuery::tz_offset_minutes` — only the
    /// `hour` group needs it.
    pub tz_offset_minutes: Option<i32>,
}

/// Network up/down totals bucketed by hour/day/week/month, mirroring
/// `get_energy_history`.
pub async fn get_network_history(
    State(state): State<Arc<AppState>>,
    Query(params): Query<NetworkHistoryQuery>,
) -> Result<Json<crate::storage::db::NetworkHistory>, StatusCode> {
    let group = params.group.as_deref().unwrap_or("hour");
    let tz = params.tz_offset_minutes.unwrap_or_else(|| {
        let secs_east = chrono::Local::now().offset().local_minus_utc();
        -(secs_east / 60)
    });
    match state.db.network_history(group, tz) {
        Ok(Some(h)) => Ok(Json(h)),
        Ok(None) => Err(StatusCode::BAD_REQUEST),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
