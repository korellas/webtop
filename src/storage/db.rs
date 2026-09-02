use crate::collector::snapshot::{AggregatedMetric, SystemSnapshot};
use chrono::{Datelike, Duration as ChronoDuration, Local, NaiveDate, TimeZone};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// Idempotency flag for the one-time pass that populates `energy_wh` for
/// rows written before the column existed. Those rows would otherwise read
/// as 0 Wh and make the hourly energy chart show empty/short bars for the
/// pre-upgrade portion of the 7-day raw window.
const META_ENERGY_WH_BACKFILL_V1: &str = "energy_wh_backfill_v1";

/// Idempotency flag for the one-time pass that rewrites the net/disk byte-rate
/// columns from raw between-tick deltas into true per-second rates. Pre-fix
/// rows stored the delta as if it accrued in exactly 1 s, inflating every rate
/// by the actual tick interval.
const META_BYTE_RATES_NORMALIZED_V1: &str = "byte_rates_normalized_v1";

/// Idempotency flag for the one-time pass that populates
/// `net_up_bytes_delta` / `net_down_bytes_delta` for rows written before
/// those columns existed. Without it, the network history's "Hour" view
/// would show empty bars for however much of the 7-day raw window predates
/// the upgrade — same failure shape `META_ENERGY_WH_BACKFILL_V1` exists to
/// avoid.
const META_NET_BYTES_DELTA_BACKFILL_V1: &str = "net_bytes_delta_backfill_v1";

/// Ceiling for the `-wal` sidecar file, applied via `PRAGMA journal_size_limit`.
/// SQLite trims the WAL back to this after each checkpoint. 16 MB leaves ample
/// headroom over the ~4 MB autocheckpoint threshold while keeping the on-disk
/// footprint predictable over years of uptime.
const WAL_SIZE_LIMIT_BYTES: i64 = 16 * 1024 * 1024;

pub struct MetricsDb {
    /// Shared with `storage::folders`, which adds the folder-size queries in
    /// its own module to keep this file focused.
    pub(super) conn: Mutex<Connection>,
}

fn summarize(group: &str, buckets: Vec<EnergyBucket>) -> EnergyHistory {
    let total_wh: f64 = buckets.iter().map(|b| b.wh).sum();
    let avg_wh = if buckets.is_empty() {
        0.0
    } else {
        total_wh / buckets.len() as f64
    };
    let peak_wh = buckets.iter().map(|b| b.wh).fold(0.0_f64, f64::max);
    EnergyHistory {
        group: group.to_string(),
        buckets,
        total_wh,
        avg_wh,
        peak_wh,
    }
}

fn format_day_key(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// Unix-ms UTC that corresponds to local 00:00 on the given date. Falling
/// back to a synthesized fixed offset handles DST-ambiguous midnights.
fn local_midnight_ms(d: NaiveDate) -> i64 {
    let ndt = d.and_hms_opt(0, 0, 0).expect("00:00 is always valid");
    match Local.from_local_datetime(&ndt).earliest() {
        Some(dt) => dt.timestamp_millis(),
        None => {
            // DST gap (e.g. local midnight skipped). Use the server's
            // current offset as a close-enough approximation.
            let offset_secs = Local::now().offset().local_minus_utc() as i64;
            ndt.and_utc().timestamp_millis() - offset_secs * 1000
        }
    }
}

/// One row of an energy-history response. `wh` is the energy consumed in
/// the bucket starting at `bucket_start` (Unix ms).
#[derive(Debug, Clone, Serialize)]
pub struct EnergyBucket {
    pub bucket_start: u64,
    pub wh: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EnergyHistory {
    pub group: String,
    pub buckets: Vec<EnergyBucket>,
    pub total_wh: f64,
    pub avg_wh: f64,
    pub peak_wh: f64,
}

/// One row of a network-history response, mirroring `EnergyBucket` but with
/// two series (up/down) since a byte total has a direction where energy
/// doesn't.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkBucket {
    pub bucket_start: u64,
    pub up_bytes: f64,
    pub down_bytes: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkHistory {
    pub group: String,
    pub buckets: Vec<NetworkBucket>,
    pub total_up_bytes: f64,
    pub total_down_bytes: f64,
}

fn summarize_network(group: &str, buckets: Vec<NetworkBucket>) -> NetworkHistory {
    let total_up_bytes: f64 = buckets.iter().map(|b| b.up_bytes).sum();
    let total_down_bytes: f64 = buckets.iter().map(|b| b.down_bytes).sum();
    NetworkHistory {
        group: group.to_string(),
        buckets,
        total_up_bytes,
        total_down_bytes,
    }
}

impl MetricsDb {
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        // A bare filename has no parent to create; that's fine, not an error.
        if let Some(dir) = Path::new(path).parent() {
            std::fs::create_dir_all(dir).ok();
        }

        let conn = Connection::open(path)?;
        // `journal_size_limit` is what actually bounds the `-wal` sidecar.
        // A checkpoint alone only *resets* the WAL — it rewinds the write
        // offset but leaves the file at its high-water mark, so one large
        // transaction (our 7-day prune) permanently inflates it. With a limit
        // set, SQLite truncates the file back down after every checkpoint.
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA journal_size_limit = {WAL_SIZE_LIMIT_BYTES};",
        ))?;

        // Raw collector snapshots — the single source of truth for history.
        // Any timescale can be derived on-the-fly by GROUP BY time bucket, so
        // we never lose data across process restarts.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metrics_raw (
                timestamp INTEGER PRIMARY KEY,
                cpu_total REAL, cpu_p_cores REAL, cpu_e_cores REAL,
                gpu_usage REAL,
                mem_used INTEGER, mem_swap_used INTEGER,
                disk_read_bytes_sec INTEGER, disk_write_bytes_sec INTEGER,
                net_up_bytes_sec INTEGER, net_down_bytes_sec INTEGER,
                power_total_w REAL, power_cpu_w REAL, power_gpu_w REAL,
                power_other_w REAL,
                cpu_temp_c REAL DEFAULT 0,
                gpu_temp_c REAL DEFAULT 0,
                fan_rpm REAL DEFAULT 0,
                energy_wh REAL DEFAULT 0,
                net_up_bytes_delta INTEGER DEFAULT 0,
                net_down_bytes_delta INTEGER DEFAULT 0
            );",
        )?;

        // Forward-compatible migration: older DBs created before these
        // columns existed need them added in place. SQLite's ALTER TABLE
        // ADD COLUMN has no IF NOT EXISTS, so we attempt the ALTER and
        // swallow the "duplicate column" error — anything else would
        // have failed the CREATE above.
        for col in ["cpu_temp_c", "gpu_temp_c", "fan_rpm", "energy_wh"] {
            let _ = conn.execute(
                &format!("ALTER TABLE metrics_raw ADD COLUMN {col} REAL DEFAULT 0"),
                [],
            );
        }
        for col in ["net_up_bytes_delta", "net_down_bytes_delta"] {
            let _ = conn.execute(
                &format!("ALTER TABLE metrics_raw ADD COLUMN {col} INTEGER DEFAULT 0"),
                [],
            );
        }

        // Legacy aggregation tables — kept so existing databases open without
        // errors. No longer written to; queries use metrics_raw instead.
        let create_legacy = |name: &str| {
            conn.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {name} (
                    timestamp INTEGER PRIMARY KEY,
                    cpu_total REAL, cpu_p_cores REAL, cpu_e_cores REAL,
                    gpu_usage REAL,
                    mem_used INTEGER, mem_swap_used INTEGER,
                    disk_read_bytes_sec INTEGER, disk_write_bytes_sec INTEGER,
                    net_up_bytes_sec INTEGER, net_down_bytes_sec INTEGER,
                    power_total_w REAL, power_cpu_w REAL, power_gpu_w REAL,
                    power_other_w REAL DEFAULT 0
                );"
            ))
        };
        create_legacy("metrics_1min")?;
        create_legacy("metrics_15min")?;
        create_legacy("metrics_1hr")?;

        Self::init_folder_schema(&conn)?;

        // Key-value meta table for small scalars that must survive restarts
        // (e.g. cumulative energy counter, monthly total).
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;

        // Per-local-day cumulative energy. ONE row per calendar day in the
        // server's local timezone. Not pruned (a year of data ≈ 365 rows).
        //
        // Why a dedicated table instead of aggregating `metrics_raw`?
        // `metrics_raw` is pruned to 7 days to keep disk usage bounded, so
        // anything longer than a week (day-tab's 30 days, week-tab's 84 days,
        // month-tab's 12 months) would silently truncate. Persisting the
        // per-day sum once a second keeps the long-term view trustworthy
        // while `metrics_raw` stays lean.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS energy_daily (
                day_key TEXT PRIMARY KEY,   -- YYYY-MM-DD in server-local TZ
                wh      REAL NOT NULL DEFAULT 0
            );",
        )?;

        // Per-local-day cumulative network bytes. Same rationale and shape as
        // `energy_daily` — `metrics_raw` only holds 7 days, so the network
        // history's Day/Week/Month tabs need a durable rollup that outlives
        // the raw retention window. Written from `insert_raw`, which already
        // computes the per-tick byte deltas for `metrics_raw` itself.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS network_daily (
                day_key    TEXT PRIMARY KEY,   -- YYYY-MM-DD in server-local TZ
                up_bytes   REAL NOT NULL DEFAULT 0,
                down_bytes REAL NOT NULL DEFAULT 0
            );",
        )?;

        crate::storage::rollup::create_schema(&conn)?;

        // The per-service sample table is gone. It fed
        // `/api/services/{name}/history`, an endpoint the dashboard never
        // called — no chart, no detail view, nothing. Nineteen services at one
        // row every five seconds had grown it to 1.89 M rows and 218 MB of a
        // 261 MB database, two thirds of that in two indexes, for a screen
        // that does not exist.
        //
        // Dropping frees the pages to the freelist but leaves the file at its
        // high-water mark, so this vacuums once to hand the space back. Both
        // steps are guarded by a meta key: a VACUUM of a quarter-gigabyte
        // costs seconds, and it must not happen on every start.
        let already_dropped: bool = conn
            .query_row(
                "SELECT 1 FROM meta WHERE key = 'service_samples_dropped_v1'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if !already_dropped {
            let present: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name='service_samples'",
                    [],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            conn.execute_batch("DROP TABLE IF EXISTS service_samples;")?;
            conn.execute(
                "INSERT OR REPLACE INTO meta(key, value) VALUES('service_samples_dropped_v1', '1')",
                [],
            )?;
            if present {
                tracing::info!("reclaiming space from the removed service_samples table");
                conn.execute_batch("VACUUM;")?;
            }
        }

        let db = Self {
            conn: Mutex::new(conn),
        };
        db.ensure_energy_wh_backfilled();
        db.ensure_byte_rates_normalized();
        // Must run after the byte-rate heal above — it reads
        // `net_up_bytes_sec` / `net_down_bytes_sec`, which that pass may just
        // have rewritten for legacy rows.
        db.ensure_net_bytes_delta_backfilled();
        Ok(db)
    }

    /// One-time heal: rewrite the net/disk byte-rate columns for rows written
    /// before the rate fix. Those rows stored a raw between-tick delta labelled
    /// "per second"; dividing each by the real interval to the previous row
    /// turns them into honest per-second rates. Runs once
    /// (meta-guarded) and — like the energy heal — only ever sees pre-fix rows,
    /// because it runs at `open()` before the collector writes any new
    /// (already-correct) rows. Rows with no predecessor or a non-positive gap
    /// are left untouched.
    fn ensure_byte_rates_normalized(&self) {
        if self.meta_get(META_BYTE_RATES_NORMALIZED_V1).is_some() {
            return;
        }
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute_batch(
                "WITH d AS (
                     SELECT timestamp,
                            (timestamp - LAG(timestamp) OVER (ORDER BY timestamp)) / 1000.0 AS dt
                     FROM metrics_raw
                 )
                 UPDATE metrics_raw
                 SET net_down_bytes_sec   = CAST(ROUND(net_down_bytes_sec   / d.dt) AS INTEGER),
                     net_up_bytes_sec     = CAST(ROUND(net_up_bytes_sec     / d.dt) AS INTEGER),
                     disk_read_bytes_sec  = CAST(ROUND(disk_read_bytes_sec  / d.dt) AS INTEGER),
                     disk_write_bytes_sec = CAST(ROUND(disk_write_bytes_sec / d.dt) AS INTEGER)
                 FROM d
                 WHERE metrics_raw.timestamp = d.timestamp
                   AND d.dt IS NOT NULL
                   AND d.dt > 0;",
            );
        }
        self.meta_set(META_BYTE_RATES_NORMALIZED_V1, "1");
    }

    /// One-time heal: fill `energy_wh` for every existing row from the real
    /// gap to the previous sample, clamped to [0, 5] s exactly like the live
    /// session counter. Runs once (guarded by a meta flag) so it can never
    /// clobber the accurate values `insert_raw` writes for live rows.
    ///
    /// Why this is needed: collector ticks are not a fixed one-second interval,
    /// so a row count is not a second count. Weighting each row by its actual
    /// interval makes `SUM(energy_wh)` a faithful Wh total instead of
    /// undercounting by the tick-interval factor.
    fn ensure_energy_wh_backfilled(&self) {
        if self.meta_get(META_ENERGY_WH_BACKFILL_V1).is_some() {
            return;
        }
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute_batch(
                "UPDATE metrics_raw
                 SET energy_wh = COALESCE((
                     SELECT metrics_raw.power_total_w * MIN(MAX(
                                (metrics_raw.timestamp - MAX(p.timestamp)) / 1000.0,
                                0.0), 5.0) / 3600.0
                     FROM metrics_raw AS p
                     WHERE p.timestamp < metrics_raw.timestamp
                 ), 0.0);",
            );
        }
        self.meta_set(META_ENERGY_WH_BACKFILL_V1, "1");
    }

    /// One-time heal: fill `net_up_bytes_delta` / `net_down_bytes_delta` for
    /// every existing row, using the same real-gap-to-previous-sample dt as
    /// `ensure_energy_wh_backfilled`. Without this, rows written before the
    /// columns existed sum to 0 bytes, and the network history's "Hour" view
    /// shows an empty bar for however much of the 7-day window predates the
    /// upgrade.
    fn ensure_net_bytes_delta_backfilled(&self) {
        if self.meta_get(META_NET_BYTES_DELTA_BACKFILL_V1).is_some() {
            return;
        }
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute_batch(
                "UPDATE metrics_raw
                 SET net_up_bytes_delta = COALESCE((
                         SELECT ROUND(metrics_raw.net_up_bytes_sec * MIN(MAX(
                                    (metrics_raw.timestamp - MAX(p.timestamp)) / 1000.0,
                                    0.0), 5.0))
                         FROM metrics_raw AS p
                         WHERE p.timestamp < metrics_raw.timestamp
                     ), 0),
                     net_down_bytes_delta = COALESCE((
                         SELECT ROUND(metrics_raw.net_down_bytes_sec * MIN(MAX(
                                    (metrics_raw.timestamp - MAX(p.timestamp)) / 1000.0,
                                    0.0), 5.0))
                         FROM metrics_raw AS p
                         WHERE p.timestamp < metrics_raw.timestamp
                     ), 0);",
            );
        }
        self.meta_set(META_NET_BYTES_DELTA_BACKFILL_V1, "1");
    }

    // -----------------------------------------------------------------------
    // Raw snapshot write / query
    // -----------------------------------------------------------------------

    /// Persist one collector snapshot. Called from the collector async task.
    pub fn insert_raw(&self, s: &SystemSnapshot) -> Result<(), rusqlite::Error> {
        let mut conn = crate::sync::guard(self.conn.lock());
        let tx = conn.transaction()?;

        // Energy attributable to THIS sample = power × (wall-clock gap to the
        // previous sample), clamped to [0, 5] s like the live counter. We
        // derive the interval from the stored timestamps rather than assuming
        // 1 Hz, because the collector uses different active and idle cadences.
        // Storing the real per-tick Wh here lets the hourly chart be a plain
        // `SUM(energy_wh)` that can't undercount.
        let prev_ts: Option<i64> =
            tx.query_row("SELECT MAX(timestamp) FROM metrics_raw", [], |r| {
                r.get::<_, Option<i64>>(0)
            })?;
        // Same dt, reused for the network byte deltas below: `net_up_bytes_sec`
        // is already this exact interval's average rate (cumulative-counter
        // delta / that interval's elapsed time), so `rate * dt` reconstructs
        // the original byte count exactly rather than approximating it —
        // unlike `energy_wh` above, where `power_total_w` is a point reading
        // and `rate * dt` is a genuine (if standard) rectangle-rule estimate.
        let dt = match prev_ts {
            Some(prev) if (s.timestamp as i64) > prev => {
                Some(((s.timestamp as i64 - prev) as f64 / 1000.0).clamp(0.0, 5.0))
            }
            _ => None,
        };
        let energy_wh = dt
            .map(|dt| (s.power_total_w as f64) * dt / 3600.0)
            .unwrap_or(0.0);
        let net_up_bytes_delta = dt
            .map(|dt| (s.net_up_bytes_sec as f64 * dt).round() as i64)
            .unwrap_or(0);
        let net_down_bytes_delta = dt
            .map(|dt| (s.net_down_bytes_sec as f64 * dt).round() as i64)
            .unwrap_or(0);

        tx.execute(
            "INSERT OR REPLACE INTO metrics_raw (
                timestamp, cpu_total, cpu_p_cores, cpu_e_cores, gpu_usage,
                mem_used, mem_swap_used,
                disk_read_bytes_sec, disk_write_bytes_sec,
                net_up_bytes_sec, net_down_bytes_sec,
                power_total_w, power_cpu_w, power_gpu_w, power_other_w,
                cpu_temp_c, gpu_temp_c, fan_rpm, energy_wh,
                net_up_bytes_delta, net_down_bytes_delta
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
            params![
                s.timestamp,
                s.cpu_total,
                s.cpu_p_cores,
                s.cpu_e_cores,
                s.gpu_usage,
                s.mem_used,
                s.mem_swap_used,
                s.disk_read_bytes_sec,
                s.disk_write_bytes_sec,
                s.net_up_bytes_sec,
                s.net_down_bytes_sec,
                s.power_total_w,
                s.power_cpu_w,
                s.power_gpu_w,
                s.power_other_w,
                s.cpu_temp_c,
                s.gpu_temp_c,
                s.fan_rpm,
                energy_wh,
                net_up_bytes_delta,
                net_down_bytes_delta,
            ],
        )?;

        // Roll the same delta into the durable per-day store, mirroring how
        // the collector accumulates `energy_daily` — done here instead
        // because both deltas are already derived from `dt` above, with no
        // need for a second source of truth in the collector tick loop.
        let day_key = Local::now().format("%Y-%m-%d").to_string();
        if net_up_bytes_delta > 0 || net_down_bytes_delta > 0 {
            tx.execute(
                "INSERT INTO network_daily(day_key, up_bytes, down_bytes) VALUES(?1, ?2, ?3)
                 ON CONFLICT(day_key) DO UPDATE SET
                     up_bytes = up_bytes + excluded.up_bytes,
                     down_bytes = down_bytes + excluded.down_bytes",
                params![
                    day_key,
                    net_up_bytes_delta as f64,
                    net_down_bytes_delta as f64
                ],
            )?;
        }

        if energy_wh > 0.0 {
            tx.execute(
                "INSERT INTO energy_daily(day_key, wh) VALUES(?1, ?2)
                 ON CONFLICT(day_key) DO UPDATE SET wh = wh + excluded.wh",
                params![day_key, energy_wh],
            )?;
        }
        tx.execute(
            "INSERT INTO meta(key, value) VALUES('energy_session_wh', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![s.energy_session_wh.to_string()],
        )?;

        tx.commit()
    }

    /// Query the raw table with time-bucketed averaging.
    ///
    /// `since` — earliest timestamp to include (Unix milliseconds).
    /// `bucket_ms` — bucket width in milliseconds; all rows whose timestamps
    /// fall in the same bucket are averaged together.
    ///
    /// Returns one `AggregatedMetric` per bucket, ordered ascending.
    pub fn query_bucketed(
        &self,
        since: u64,
        bucket_ms: u64,
    ) -> Result<Vec<AggregatedMetric>, rusqlite::Error> {
        let conn = crate::sync::guard(self.conn.lock());
        let mut stmt = conn.prepare(
            "SELECT
                (timestamp / ?1) * ?1 AS bucket,
                AVG(cpu_total), AVG(cpu_p_cores), AVG(cpu_e_cores), AVG(gpu_usage),
                CAST(AVG(CAST(mem_used        AS REAL)) AS INTEGER),
                CAST(AVG(CAST(mem_swap_used   AS REAL)) AS INTEGER),
                CAST(AVG(CAST(disk_read_bytes_sec  AS REAL)) AS INTEGER),
                CAST(AVG(CAST(disk_write_bytes_sec AS REAL)) AS INTEGER),
                CAST(AVG(CAST(net_up_bytes_sec     AS REAL)) AS INTEGER),
                CAST(AVG(CAST(net_down_bytes_sec   AS REAL)) AS INTEGER),
                AVG(power_total_w), AVG(power_cpu_w),
                AVG(power_gpu_w),   AVG(power_other_w),
                AVG(cpu_temp_c),    AVG(gpu_temp_c),
                AVG(fan_rpm),
                -- Per-bucket extremes for the charted series. Averages alone
                -- hide saturation: a 2 s spike to 100 % inside a 4-minute
                -- bucket averages to under 1 %. The chart draws [min, max] as
                -- a band behind the mean so peaks survive every timescale.
                MIN(cpu_total),          MAX(cpu_total),
                MIN(gpu_usage),          MAX(gpu_usage),
                MIN(mem_used),           MAX(mem_used),
                MIN(power_total_w),      MAX(power_total_w),
                MIN(net_up_bytes_sec),   MAX(net_up_bytes_sec),
                MIN(net_down_bytes_sec), MAX(net_down_bytes_sec),
                MIN(disk_read_bytes_sec),  MAX(disk_read_bytes_sec),
                MIN(disk_write_bytes_sec), MAX(disk_write_bytes_sec),
                MIN(cpu_temp_c),         MAX(cpu_temp_c)
             FROM metrics_raw
             WHERE timestamp >= ?2
             GROUP BY bucket
             ORDER BY bucket ASC",
        )?;
        let rows = stmt.query_map(params![bucket_ms, since], |row| {
            Ok(AggregatedMetric {
                timestamp: row.get(0)?,
                cpu_total: row.get(1)?,
                cpu_p_cores: row.get(2)?,
                cpu_e_cores: row.get(3)?,
                gpu_usage: row.get(4)?,
                mem_used: row.get(5)?,
                mem_swap_used: row.get(6)?,
                disk_read_bytes_sec: row.get(7)?,
                disk_write_bytes_sec: row.get(8)?,
                net_up_bytes_sec: row.get(9)?,
                net_down_bytes_sec: row.get(10)?,
                power_total_w: row.get(11)?,
                power_cpu_w: row.get(12)?,
                power_gpu_w: row.get(13)?,
                power_other_w: row.get(14)?,
                // Older rows that pre-date the temperature/fan columns
                // come back as NULL — fall back to 0.0 so deserialisation
                // is infallible.
                cpu_temp_c: row.get::<_, Option<f32>>(15)?.unwrap_or(0.0),
                gpu_temp_c: row.get::<_, Option<f32>>(16)?.unwrap_or(0.0),
                fan_rpm: row.get::<_, Option<f32>>(17)?.unwrap_or(0.0),
                band: crate::collector::snapshot::MetricBand {
                    cpu_total: [row.get(18)?, row.get(19)?],
                    gpu_usage: [row.get(20)?, row.get(21)?],
                    mem_used: [row.get(22)?, row.get(23)?],
                    power_total_w: [row.get(24)?, row.get(25)?],
                    net_up_bytes_sec: [row.get(26)?, row.get(27)?],
                    net_down_bytes_sec: [row.get(28)?, row.get(29)?],
                    disk_read_bytes_sec: [row.get(30)?, row.get(31)?],
                    disk_write_bytes_sec: [row.get(32)?, row.get(33)?],
                    // Same NULL caveat as the averages above — rows written
                    // before the temperature column existed have no value.
                    cpu_temp_c: [
                        row.get::<_, Option<f32>>(34)?.unwrap_or(0.0),
                        row.get::<_, Option<f32>>(35)?.unwrap_or(0.0),
                    ],
                },
            })
        })?;
        rows.collect()
    }

    /// Total bytes transferred since `since`, summed from the per-tick deltas
    /// `insert_raw` already computed.
    ///
    /// Deliberately does NOT reuse `query_bucketed`'s bucket averages —
    /// `AVG(rate) * bucket_width` would compound the bucket mean's rounding
    /// with whatever the chart's own downsample/smooth passes do to it before
    /// a total is ever read off. And unlike a generic rate signal, no
    /// integration happens here *at query time* at all: `net_up_bytes_delta`
    /// already **is** each row's byte count (see `insert_raw`), so the exact
    /// total is a plain `SUM`.
    pub fn net_totals(&self, since: u64) -> Result<(f64, f64), rusqlite::Error> {
        let conn = crate::sync::guard(self.conn.lock());
        conn.query_row(
            "SELECT
                 COALESCE(SUM(net_up_bytes_delta), 0),
                 COALESCE(SUM(net_down_bytes_delta), 0)
             FROM metrics_raw
             WHERE timestamp >= ?1",
            params![since],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    }

    /// Network up/down history bucketed by `group`, mirroring `energy_history`.
    ///
    /// `hour` (24 × 1h) derives on-the-fly from `metrics_raw` — 24h fits
    /// inside the 7-day retention window, so this reads the freshest possible
    /// number for the current hour. `day` / `week` / `month` read from
    /// `network_daily`, which — like `energy_daily` — accumulates one row per
    /// local calendar day and outlives `metrics_raw`'s 7-day prune.
    pub fn network_history(
        &self,
        group: &str,
        tz_offset_minutes: i32,
    ) -> Result<Option<NetworkHistory>, rusqlite::Error> {
        match group {
            "hour" => self.network_history_hour(tz_offset_minutes),
            "day" => self.network_history_daily(30),
            "week" => self.network_history_weekly(12),
            "month" => self.network_history_monthly(12),
            _ => return Ok(None),
        }
        .map(Some)
    }

    /// 24 × 1h buckets, aligned to the caller's local hour — same alignment
    /// math as `energy_history_hour`.
    fn network_history_hour(
        &self,
        tz_offset_minutes: i32,
    ) -> Result<NetworkHistory, rusqlite::Error> {
        let bucket_ms: i64 = 3_600_000;
        let count: i64 = 24;
        let offset_ms: i64 = -(tz_offset_minutes as i64) * 60_000;

        let now_ms_utc = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let now_local = now_ms_utc + offset_ms;
        let aligned_end_local = (now_local / bucket_ms + 1) * bucket_ms;
        let since_local = aligned_end_local - bucket_ms * count;
        let since_utc = since_local - offset_ms;

        let conn = crate::sync::guard(self.conn.lock());
        let mut stmt = conn.prepare(
            "SELECT
                ((timestamp + ?3) / ?1) * ?1 - ?3 AS bucket,
                SUM(net_up_bytes_delta), SUM(net_down_bytes_delta)
             FROM metrics_raw
             WHERE timestamp >= ?2
             GROUP BY bucket
             ORDER BY bucket ASC",
        )?;
        let mut sums: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
        let rows = stmt.query_map(params![bucket_ms, since_utc, offset_ms], |row| {
            let b: i64 = row.get(0)?;
            let up: f64 = row.get(1).unwrap_or(0.0);
            let down: f64 = row.get(2).unwrap_or(0.0);
            Ok((b, up, down))
        })?;
        for r in rows {
            let (b, up, down) = r?;
            sums.insert(b, (up, down));
        }

        let mut buckets = Vec::with_capacity(count as usize);
        for i in 0..count {
            let local_start = since_local + i * bucket_ms;
            let b_utc = local_start - offset_ms;
            let (up, down) = sums.get(&b_utc).copied().unwrap_or((0.0, 0.0));
            buckets.push(NetworkBucket {
                bucket_start: b_utc.max(0) as u64,
                up_bytes: up,
                down_bytes: down,
            });
        }

        Ok(summarize_network("hour", buckets))
    }

    /// `count` most-recent local-calendar days, newest last.
    fn network_history_daily(&self, count: u32) -> Result<NetworkHistory, rusqlite::Error> {
        let daily = self.list_network_daily()?;
        let today = Local::now().date_naive();
        let mut buckets = Vec::with_capacity(count as usize);
        for i in (0..count).rev() {
            let d = today - ChronoDuration::days(i as i64);
            let key = format_day_key(d);
            let (up, down) = daily.get(&key).copied().unwrap_or((0.0, 0.0));
            buckets.push(NetworkBucket {
                bucket_start: local_midnight_ms(d) as u64,
                up_bytes: up,
                down_bytes: down,
            });
        }
        Ok(summarize_network("day", buckets))
    }

    /// `count` most-recent ISO weeks (Monday-start), newest last.
    fn network_history_weekly(&self, count: u32) -> Result<NetworkHistory, rusqlite::Error> {
        let daily = self.list_network_daily()?;
        let today = Local::now().date_naive();
        let days_since_monday = today.weekday().num_days_from_monday();
        let this_monday = today - ChronoDuration::days(days_since_monday as i64);

        let mut buckets = Vec::with_capacity(count as usize);
        for i in (0..count).rev() {
            let monday = this_monday - ChronoDuration::days(7 * i as i64);
            let (mut up, mut down) = (0.0, 0.0);
            for d_off in 0..7 {
                let d = monday + ChronoDuration::days(d_off);
                if let Some((u, dn)) = daily.get(&format_day_key(d)) {
                    up += u;
                    down += dn;
                }
            }
            buckets.push(NetworkBucket {
                bucket_start: local_midnight_ms(monday) as u64,
                up_bytes: up,
                down_bytes: down,
            });
        }
        Ok(summarize_network("week", buckets))
    }

    /// `count` most-recent **calendar** months, newest last.
    fn network_history_monthly(&self, count: u32) -> Result<NetworkHistory, rusqlite::Error> {
        let daily = self.list_network_daily()?;
        let today = Local::now().date_naive();
        let cur_y = today.year();
        let cur_m = today.month() as i32;

        let mut buckets = Vec::with_capacity(count as usize);
        for i in (0..count as i32).rev() {
            let total = cur_y * 12 + (cur_m - 1) - i;
            let y = total.div_euclid(12);
            let m = (total.rem_euclid(12) + 1) as u32;
            let prefix = format!("{:04}-{:02}-", y, m);
            let (mut up, mut down) = (0.0, 0.0);
            for (k, (u, dn)) in daily.iter() {
                if k.starts_with(&prefix) {
                    up += u;
                    down += dn;
                }
            }
            let first = NaiveDate::from_ymd_opt(y, m, 1).expect("valid Y-M-1");
            buckets.push(NetworkBucket {
                bucket_start: local_midnight_ms(first) as u64,
                up_bytes: up,
                down_bytes: down,
            });
        }
        Ok(summarize_network("month", buckets))
    }

    /// All `network_daily` rows, keyed by `YYYY-MM-DD`.
    fn list_network_daily(&self) -> Result<BTreeMap<String, (f64, f64)>, rusqlite::Error> {
        let conn = crate::sync::guard(self.conn.lock());
        let mut stmt = conn.prepare("SELECT day_key, up_bytes, down_bytes FROM network_daily")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, f64>(1)?,
                r.get::<_, f64>(2)?,
            ))
        })?;
        let mut map = BTreeMap::new();
        for r in rows {
            let (k, up, down) = r?;
            map.insert(k, (up, down));
        }
        Ok(map)
    }

    /// Fill in `network_daily` rows for any **completed** local day that has
    /// data in `metrics_raw` but no corresponding row yet — mirrors
    /// `backfill_energy_daily` exactly, including leaving today to the live
    /// per-tick accumulation in `insert_raw` so the two can't race.
    pub fn backfill_network_daily(&self, today_key: &str) -> Result<usize, rusqlite::Error> {
        let conn = crate::sync::guard(self.conn.lock());
        conn.execute(
            "INSERT INTO network_daily(day_key, up_bytes, down_bytes)
             SELECT d, up, down FROM (
                 SELECT
                     date(timestamp/1000, 'unixepoch', 'localtime') AS d,
                     SUM(net_up_bytes_delta) AS up,
                     SUM(net_down_bytes_delta) AS down
                 FROM metrics_raw
                 GROUP BY d
             )
             WHERE d != ?1
             ON CONFLICT(day_key) DO NOTHING",
            params![today_key],
        )
    }

    /// Remove rows from `metrics_raw` older than `before` (Unix milliseconds).
    pub fn prune_raw(&self, before: u64) -> Result<usize, rusqlite::Error> {
        let conn = crate::sync::guard(self.conn.lock());
        conn.execute(
            "DELETE FROM metrics_raw WHERE timestamp < ?1",
            params![before],
        )
    }

    /// Fold the WAL back into the main database and shrink the sidecar file
    /// to nothing.
    ///
    /// SQLite's automatic checkpoints already run at ~4 MB of WAL, but they
    /// are best-effort: an overlapping reader makes them give up, and the
    /// process can then run for a long time with a WAL that only ever grows.
    /// Running an explicit `TRUNCATE` checkpoint after each prune — the point
    /// where we've just generated the largest write of the cycle — keeps that
    /// from accumulating.
    pub fn checkpoint(&self) -> Result<(), rusqlite::Error> {
        let conn = crate::sync::guard(self.conn.lock());
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
    }

    /// Compute energy consumption bucketed by the requested grouping.
    ///
    /// Data source depends on the time horizon:
    ///   • `hour` (24 × 1h) → derived on-the-fly from `metrics_raw` — 24h
    ///     fits inside the 7-day retention window, and falling back to raw
    ///     data gives the freshest possible number for the current hour.
    ///   • `day` / `week` / `month` → derived from `energy_daily`, a table
    ///     that accumulates one row per local calendar day. Rolling 30/84/365
    ///     day totals would silently truncate against `metrics_raw` (which is
    ///     capped at 7 days), so long-term views MUST read from the durable
    ///     daily store instead.
    ///
    /// `tz_offset_minutes` matches JavaScript `Date.getTimezoneOffset()`
    /// (minutes the local TZ is *behind* UTC; KST → -540). Only the hour
    /// view needs it — the daily/weekly/monthly views key off server-local
    /// dates, which by construction match the browser's TZ for a machine
    /// the user is sitting in front of.
    /// `Ok(None)` means the group name is not one we support — the caller
    /// should reject the request rather than render an empty chart.
    pub fn energy_history(
        &self,
        group: &str,
        tz_offset_minutes: i32,
    ) -> Result<Option<EnergyHistory>, rusqlite::Error> {
        match group {
            "hour" => self.energy_history_hour(tz_offset_minutes),
            "day" => self.energy_history_daily(30),
            "week" => self.energy_history_weekly(12),
            "month" => self.energy_history_monthly(12),
            // Unknown group is a caller error, not an empty dataset. This used
            // to return an all-zero history, which rendered as a blank but
            // *successful* chart — indistinguishable from "no energy was used".
            // `None` lets the handler answer 400 instead, matching how
            // `/api/history` already treats an unknown range.
            _ => return Ok(None),
        }
        .map(Some)
    }

    /// 24 × 1h buckets, aligned to the caller's local hour.
    fn energy_history_hour(
        &self,
        tz_offset_minutes: i32,
    ) -> Result<EnergyHistory, rusqlite::Error> {
        let bucket_ms: i64 = 3_600_000;
        let count: i64 = 24;

        let offset_ms: i64 = -(tz_offset_minutes as i64) * 60_000;

        let now_ms_utc = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let now_local = now_ms_utc + offset_ms;
        let aligned_end_local = (now_local / bucket_ms + 1) * bucket_ms;
        let since_local = aligned_end_local - bucket_ms * count;
        let since_utc = since_local - offset_ms;

        let conn = crate::sync::guard(self.conn.lock());
        let mut stmt = conn.prepare(
            "SELECT
                ((timestamp + ?3) / ?1) * ?1 - ?3 AS bucket,
                SUM(energy_wh) AS wh
             FROM metrics_raw
             WHERE timestamp >= ?2
             GROUP BY bucket
             ORDER BY bucket ASC",
        )?;
        let mut sums: BTreeMap<i64, f64> = BTreeMap::new();
        let rows = stmt.query_map(params![bucket_ms, since_utc, offset_ms], |row| {
            let b: i64 = row.get(0)?;
            let s: f64 = row.get(1).unwrap_or(0.0);
            Ok((b, s))
        })?;
        for r in rows {
            let (b, wh) = r?;
            // `energy_wh` is already per-sample Wh (power × actual interval),
            // so the bucket total is a straight sum — no 1 Hz assumption.
            sums.insert(b, wh);
        }

        let mut buckets = Vec::with_capacity(count as usize);
        for i in 0..count {
            let local_start = since_local + i * bucket_ms;
            let b_utc = local_start - offset_ms;
            buckets.push(EnergyBucket {
                bucket_start: b_utc.max(0) as u64,
                wh: sums.get(&b_utc).copied().unwrap_or(0.0),
            });
        }

        Ok(summarize("hour", buckets))
    }

    /// `count` most-recent local-calendar days, newest last.
    fn energy_history_daily(&self, count: u32) -> Result<EnergyHistory, rusqlite::Error> {
        let daily = self.list_energy_daily()?;
        let today = Local::now().date_naive();
        let mut buckets = Vec::with_capacity(count as usize);
        for i in (0..count).rev() {
            let d = today - ChronoDuration::days(i as i64);
            let key = format_day_key(d);
            let wh = daily.get(&key).copied().unwrap_or(0.0);
            buckets.push(EnergyBucket {
                bucket_start: local_midnight_ms(d) as u64,
                wh,
            });
        }
        Ok(summarize("day", buckets))
    }

    /// `count` most-recent ISO weeks (Monday-start), newest last. Each bucket's
    /// value is the sum of the 7 daily rows in that week.
    fn energy_history_weekly(&self, count: u32) -> Result<EnergyHistory, rusqlite::Error> {
        let daily = self.list_energy_daily()?;
        let today = Local::now().date_naive();
        // Back up to the most recent Monday (today if already Monday).
        let days_since_monday = today.weekday().num_days_from_monday();
        let this_monday = today - ChronoDuration::days(days_since_monday as i64);

        let mut buckets = Vec::with_capacity(count as usize);
        for i in (0..count).rev() {
            let monday = this_monday - ChronoDuration::days(7 * i as i64);
            let mut wh = 0.0;
            for d_off in 0..7 {
                let d = monday + ChronoDuration::days(d_off);
                if let Some(v) = daily.get(&format_day_key(d)) {
                    wh += v;
                }
            }
            buckets.push(EnergyBucket {
                bucket_start: local_midnight_ms(monday) as u64,
                wh,
            });
        }
        Ok(summarize("week", buckets))
    }

    /// `count` most-recent **calendar** months (e.g. April 2026), newest last.
    /// Sums every daily row whose key starts with the month's "YYYY-MM".
    fn energy_history_monthly(&self, count: u32) -> Result<EnergyHistory, rusqlite::Error> {
        let daily = self.list_energy_daily()?;
        let today = Local::now().date_naive();
        let cur_y = today.year();
        let cur_m = today.month() as i32; // 1..=12

        let mut buckets = Vec::with_capacity(count as usize);
        for i in (0..count as i32).rev() {
            // Step back `i` calendar months from the current one.
            let total = cur_y * 12 + (cur_m - 1) - i;
            let y = total.div_euclid(12);
            let m = (total.rem_euclid(12) + 1) as u32;
            let prefix = format!("{:04}-{:02}-", y, m);
            let wh: f64 = daily
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(_, v)| *v)
                .sum();
            let first = NaiveDate::from_ymd_opt(y, m, 1).expect("valid Y-M-1");
            buckets.push(EnergyBucket {
                bucket_start: local_midnight_ms(first) as u64,
                wh,
            });
        }
        Ok(summarize("month", buckets))
    }

    // -----------------------------------------------------------------------
    // Daily energy store — writes from collector, reads from energy_history.
    // -----------------------------------------------------------------------

    /// Add `delta_wh` to the current local-day's bucket, creating the row if
    /// it doesn't exist. Called from the collector tick every ~1s with
    /// `power_total_w × dt_secs / 3600`.
    pub fn add_daily_energy(&self, day_key: &str, delta_wh: f64) {
        if !delta_wh.is_finite() || delta_wh <= 0.0 {
            return;
        }
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "INSERT INTO energy_daily(day_key, wh) VALUES(?1, ?2)
                 ON CONFLICT(day_key) DO UPDATE SET wh = wh + excluded.wh",
                params![day_key, delta_wh],
            );
        }
    }

    /// Fill in `energy_daily` rows for any **completed** local day that has
    /// data in `metrics_raw` but no corresponding daily row yet. Called once
    /// at startup so a fresh upgrade doesn't show empty bars for the last
    /// week of data that already exists in `metrics_raw`.
    ///
    /// Today is intentionally excluded: the live collector tick owns today's
    /// row, and the backfill would otherwise race with live accumulation.
    pub fn backfill_energy_daily(&self, today_key: &str) -> Result<usize, rusqlite::Error> {
        let conn = crate::sync::guard(self.conn.lock());
        conn.execute(
            "INSERT INTO energy_daily(day_key, wh)
             SELECT d, w FROM (
                 SELECT
                     date(timestamp/1000, 'unixepoch', 'localtime') AS d,
                     SUM(energy_wh) AS w
                 FROM metrics_raw
                 GROUP BY d
             )
             WHERE d != ?1
             ON CONFLICT(day_key) DO NOTHING",
            params![today_key],
        )
    }

    /// Sum of `energy_daily.wh` for rows whose `day_key` starts with the given
    /// `YYYY-MM` prefix. Used by the collector's startup reconciliation to
    /// detect a gap between the cumulative session counter (meta) and the
    /// durable daily store (this table).
    pub fn sum_energy_daily_for_month(&self, month_prefix: &str) -> f64 {
        let Ok(conn) = self.conn.lock() else {
            return 0.0;
        };
        conn.query_row(
            "SELECT COALESCE(SUM(wh), 0) FROM energy_daily
             WHERE day_key LIKE ?1 || '%'",
            params![month_prefix],
            |r| r.get::<_, f64>(0),
        )
        .unwrap_or(0.0)
    }

    /// All daily rows, keyed by `YYYY-MM-DD`. Small (<1000 rows even after
    /// years of uptime), so we just read the whole thing and index in Rust.
    fn list_energy_daily(&self) -> Result<BTreeMap<String, f64>, rusqlite::Error> {
        let conn = crate::sync::guard(self.conn.lock());
        let mut stmt = conn.prepare("SELECT day_key, wh FROM energy_daily")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))?;
        let mut map = BTreeMap::new();
        for r in rows {
            let (k, v) = r?;
            map.insert(k, v);
        }
        Ok(map)
    }

    // -----------------------------------------------------------------------
    // Meta key-value store
    // -----------------------------------------------------------------------

    /// Fetch a scalar from the `meta` table. Returns `None` on miss or error.
    pub fn meta_get(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().ok()?;
        conn.query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .ok()
    }

    /// Upsert a scalar into the `meta` table. Errors are swallowed — this is
    /// best-effort (a transient DB hiccup shouldn't crash the collector).
    pub fn meta_set(&self, key: &str, value: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "INSERT INTO meta(key, value) VALUES(?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests run in parallel threads, so the database path has to be unique
    /// per call. A wall-clock timestamp is not enough — two threads entering
    /// within the same clock tick get the same filename and then silently
    /// share a database, which shows up as another test's rows appearing in
    /// yours. A process-wide counter is exact.
    fn mk_db() -> MetricsDb {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = format!("/tmp/webtop-test-{}-{n}.db", std::process::id());
        // A leftover file from a previous run with a recycled pid would carry
        // its rows into this test.
        let _ = std::fs::remove_file(&path);
        MetricsDb::open(&path).expect("open test db")
    }

    /// A sample's stored energy must reflect the ACTUAL gap to the previous
    /// sample, not a 1 Hz assumption.
    #[test]
    fn insert_raw_weights_energy_by_actual_interval() {
        let db = mk_db();
        let mut s = SystemSnapshot {
            power_total_w: 10.0,
            ..Default::default()
        };
        // First row has no predecessor → 0 Wh (no interval to attribute).
        s.timestamp = 1_000_000;
        db.insert_raw(&s).unwrap();
        // Second row 2 s later → 10 W × 2 s = 0.005555… Wh.
        s.timestamp = 1_002_000;
        db.insert_raw(&s).unwrap();

        let conn = db.conn.lock().unwrap();
        let e: f64 = conn
            .query_row(
                "SELECT energy_wh FROM metrics_raw WHERE timestamp = 1002000",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        let expected = 10.0 * 2.0 / 3600.0;
        assert!(
            (e - expected).abs() < 1e-9,
            "expected {expected}, got {e} (a 1 Hz assumption would give {})",
            10.0 / 3600.0
        );
    }

    /// 10 samples 1.8 s apart at a flat 8 W should total ≈ 8 W × (9 × 1.8 s),
    /// i.e. energy follows elapsed wall-clock — NOT 8 W × 10 rows / 3600.
    #[test]
    fn insert_raw_energy_tracks_elapsed_not_row_count() {
        let db = mk_db();
        let mut s = SystemSnapshot {
            power_total_w: 8.0,
            ..Default::default()
        };
        for i in 0..10 {
            s.timestamp = 1_000_000 + i * 1_800; // 1.8 s spacing
            db.insert_raw(&s).unwrap();
        }
        let conn = db.conn.lock().unwrap();
        let total: f64 = conn
            .query_row("SELECT SUM(energy_wh) FROM metrics_raw", [], |r| r.get(0))
            .unwrap();
        drop(conn);
        let expected = 8.0 * (9.0 * 1.8) / 3600.0; // 9 intervals
        let buggy = 8.0 * 10.0 / 3600.0; // the old SUM(power)/3600
        assert!(
            (total - expected).abs() < 1e-9,
            "expected {expected}, got {total} (buggy 1 Hz total would be {buggy})"
        );
    }

    /// Rows written before the `energy_wh` column existed (energy_wh = 0) get
    /// healed once from their stored timestamps, weighted by real interval.
    #[test]
    fn legacy_backfill_weights_energy_by_actual_interval() {
        let db = mk_db();
        {
            let conn = db.conn.lock().unwrap();
            for (ts, p) in [
                (1_000_000i64, 10.0f64),
                (1_002_000, 10.0),
                (1_004_000, 10.0),
            ] {
                conn.execute(
                    "INSERT INTO metrics_raw(timestamp, power_total_w, energy_wh)
                     VALUES(?1, ?2, 0)",
                    params![ts, p],
                )
                .unwrap();
            }
            // open() already ran (and flagged) the backfill on the empty DB;
            // clear the flag so it runs again over our legacy rows.
            conn.execute(
                "DELETE FROM meta WHERE key = ?1",
                params![META_ENERGY_WH_BACKFILL_V1],
            )
            .unwrap();
        }

        db.ensure_energy_wh_backfilled();

        let conn = db.conn.lock().unwrap();
        let total: f64 = conn
            .query_row("SELECT SUM(energy_wh) FROM metrics_raw", [], |r| r.get(0))
            .unwrap();
        drop(conn);
        // row1: no predecessor → 0; row2 & row3: 10 W × 2 s each.
        let expected = 2.0 * (10.0 * 2.0 / 3600.0);
        assert!(
            (total - expected).abs() < 1e-9,
            "expected {expected}, got {total}"
        );
    }

    /// Rows written before `net_up_bytes_delta`/`net_down_bytes_delta`
    /// existed (both 0) get healed once from their stored rate and the real
    /// gap to the previous row — same shape as the energy_wh legacy heal.
    #[test]
    fn legacy_backfill_computes_net_bytes_delta_from_rate_and_real_interval() {
        let db = mk_db();
        {
            let conn = db.conn.lock().unwrap();
            for (ts, up) in [(1_000_000i64, 100i64), (1_002_000, 100), (1_004_000, 100)] {
                conn.execute(
                    "INSERT INTO metrics_raw(timestamp, net_up_bytes_sec) VALUES(?1, ?2)",
                    params![ts, up],
                )
                .unwrap();
            }
            // open() already ran (and flagged) the backfill on the empty DB;
            // clear the flag so it runs again over our legacy rows.
            conn.execute(
                "DELETE FROM meta WHERE key = ?1",
                params![META_NET_BYTES_DELTA_BACKFILL_V1],
            )
            .unwrap();
        }

        db.ensure_net_bytes_delta_backfilled();

        let conn = db.conn.lock().unwrap();
        let total: i64 = conn
            .query_row("SELECT SUM(net_up_bytes_delta) FROM metrics_raw", [], |r| {
                r.get(0)
            })
            .unwrap();
        drop(conn);
        // row1: no predecessor -> 0; row2 & row3: 100 bytes/s x 2 s each.
        let expected = 2 * (100 * 2);
        assert_eq!(total, expected, "expected {expected}, got {total}");
    }

    /// Pre-fix rate rows (raw between-tick deltas) get divided by the real
    /// interval to the previous row; the first row (no predecessor) is left
    /// untouched.
    #[test]
    fn byte_rate_normalization_divides_by_real_interval() {
        let db = mk_db();
        {
            let conn = db.conn.lock().unwrap();
            for (ts, nd) in [(1_000_000i64, 200u64), (1_002_000, 200), (1_004_000, 200)] {
                conn.execute(
                    "INSERT INTO metrics_raw(timestamp, net_down_bytes_sec) VALUES(?1, ?2)",
                    params![ts, nd],
                )
                .unwrap();
            }
            // open() already ran (and flagged) the heal on the empty DB; clear
            // the flag so it runs again over our pre-fix rows.
            conn.execute(
                "DELETE FROM meta WHERE key = ?1",
                params![META_BYTE_RATES_NORMALIZED_V1],
            )
            .unwrap();
        }

        db.ensure_byte_rates_normalized();

        let conn = db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT net_down_bytes_sec FROM metrics_raw ORDER BY timestamp")
            .unwrap();
        let rows: Vec<i64> = stmt
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        drop(stmt);
        drop(conn);
        // row1: no predecessor → unchanged; row2 & row3: 200 / 2 s = 100.
        assert_eq!(rows, vec![200, 100, 100]);
    }

    #[test]
    fn add_daily_energy_accumulates_in_same_day() {
        let db = mk_db();
        db.add_daily_energy("2026-04-21", 10.0);
        db.add_daily_energy("2026-04-21", 2.5);
        db.add_daily_energy("2026-04-21", 0.1);
        let map = db.list_energy_daily().unwrap();
        let v = map.get("2026-04-21").copied().unwrap_or(0.0);
        assert!((v - 12.6).abs() < 1e-6, "expected 12.6, got {v}");
    }

    #[test]
    fn add_daily_energy_rejects_nonpositive() {
        let db = mk_db();
        db.add_daily_energy("2026-04-21", 0.0);
        db.add_daily_energy("2026-04-21", -1.0);
        db.add_daily_energy("2026-04-21", f64::NAN);
        assert!(db.list_energy_daily().unwrap().is_empty());
    }

    #[test]
    fn monthly_buckets_cross_year_boundary() {
        // Manually insert daily rows spanning Dec 2025 → Feb 2026 and make sure
        // the monthly grouper attributes them to the correct calendar months
        // (including the year-rollover edge).
        let db = mk_db();
        db.add_daily_energy("2025-12-30", 100.0);
        db.add_daily_energy("2025-12-31", 50.0);
        db.add_daily_energy("2026-01-01", 200.0);
        db.add_daily_energy("2026-02-15", 300.0);

        let daily = db.list_energy_daily().unwrap();
        // Simulate the grouper directly so the assertion doesn't depend on "today".
        let sum_prefix = |p: &str| -> f64 {
            daily
                .iter()
                .filter(|(k, _)| k.starts_with(p))
                .map(|(_, v)| *v)
                .sum()
        };
        assert!((sum_prefix("2025-12-") - 150.0).abs() < 1e-6);
        assert!((sum_prefix("2026-01-") - 200.0).abs() < 1e-6);
        assert!((sum_prefix("2026-02-") - 300.0).abs() < 1e-6);
    }

    /// `insert_raw` must store each row's byte delta as `net_up_bytes_sec *
    /// dt` using THAT row's own rate — not averaged with the previous
    /// sample's. `net_up_bytes_sec` is already the exact average rate over
    /// its own interval (delta / that interval's elapsed time), so this
    /// reconstructs the original byte count exactly rather than
    /// approximating it, and `net_totals` is then a plain `SUM`.
    #[test]
    fn insert_raw_computes_byte_delta_from_current_rate_times_dt() {
        let db = mk_db();
        let mut s = SystemSnapshot::default();
        // Rate ramps 100 -> 300 -> 100 bytes/sec, 2 s apart.
        for (ts, up) in [(1_000_000u64, 100u64), (1_002_000, 300), (1_004_000, 100)] {
            s.timestamp = ts;
            s.net_up_bytes_sec = up;
            db.insert_raw(&s).unwrap();
        }
        let (up_bytes, _) = db.net_totals(1_000_000).unwrap();
        // Row 1: no predecessor -> 0. Row 2: 300 * 2s = 600. Row 3: 100 * 2s = 200.
        let expected = 0.0 + 600.0 + 200.0;
        assert!(
            (up_bytes - expected).abs() < 1e-9,
            "expected {expected}, got {up_bytes}"
        );
    }

    /// Each delta is attributed to its own row's timestamp (unlike a
    /// trapezoid spanning two rows), so `since` behaves like every other
    /// range query here: a row counts iff its own timestamp is `>= since`.
    #[test]
    fn net_totals_since_only_counts_rows_at_or_after_the_cutoff() {
        let db = mk_db();
        let mut s = SystemSnapshot::default();
        // Gaps stay under the [0, 5] s clamp so dt == the raw timestamp gap.
        for (ts, up) in [(1_000_000u64, 100u64), (1_002_000, 100), (1_006_000, 100)] {
            s.timestamp = ts;
            s.net_up_bytes_sec = up;
            db.insert_raw(&s).unwrap();
        }
        // since = 1_004_000 excludes rows 1 and 2; only row 3 (100 * 4s) counts.
        let (up_bytes, _) = db.net_totals(1_004_000).unwrap();
        let expected = 100.0 * 4.0;
        assert!(
            (up_bytes - expected).abs() < 1e-9,
            "expected {expected}, got {up_bytes}"
        );
    }

    /// `insert_raw` must roll the same delta into `network_daily`, mirroring
    /// how `energy_daily` accumulates — this is what backs the network
    /// history's Day/Week/Month tabs once `metrics_raw` prunes past 7 days.
    #[test]
    fn insert_raw_accumulates_network_daily() {
        let db = mk_db();
        let mut s = SystemSnapshot::default();
        for (ts, up, down) in [
            (1_000_000u64, 100u64, 50u64),
            (1_002_000, 300, 150),
            (1_004_000, 100, 50),
        ] {
            s.timestamp = ts;
            s.net_up_bytes_sec = up;
            s.net_down_bytes_sec = down;
            db.insert_raw(&s).unwrap();
        }
        let daily = db.list_network_daily().unwrap();
        assert_eq!(
            daily.len(),
            1,
            "all three samples fall on the same local day"
        );
        let (up, down) = *daily.values().next().unwrap();
        assert!((up - 800.0).abs() < 1e-9, "expected 800 up, got {up}");
        assert!((down - 400.0).abs() < 1e-9, "expected 400 down, got {down}");
    }

    #[test]
    fn insert_raw_persists_live_and_daily_energy_together() {
        let db = mk_db();
        let mut sample = SystemSnapshot {
            power_total_w: 18.0,
            energy_session_wh: 42.5,
            ..Default::default()
        };
        sample.timestamp = 1_000_000;
        db.insert_raw(&sample).unwrap();
        sample.timestamp = 1_002_000;
        db.insert_raw(&sample).unwrap();

        assert_eq!(db.meta_get("energy_session_wh").as_deref(), Some("42.5"));
        let daily_total: f64 = db.list_energy_daily().unwrap().values().sum();
        let expected = 18.0 * 2.0 / 3600.0;
        assert!((daily_total - expected).abs() < 1e-9);
    }

    #[test]
    fn backfill_skips_today() {
        let db = mk_db();
        // Seed metrics_raw with one full Wh for today and one for yesterday.
        // Pick fixed UTC instants; 'localtime' in the SQL will bucket them
        // under whatever the test host's TZ happens to be. The assertion
        // only checks that (a) something gets inserted, (b) the today_key
        // string we pass is NOT inserted.
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO metrics_raw(timestamp, power_total_w) VALUES(?1, 3600.0)",
            params![1_700_000_000_000i64], // 2023-11-14 ≈
        )
        .unwrap();
        drop(conn);

        let fake_today = "2099-01-01";
        let inserted = db.backfill_energy_daily(fake_today).unwrap();
        assert!(inserted >= 1);
        let map = db.list_energy_daily().unwrap();
        assert!(!map.contains_key(fake_today));
    }
}
