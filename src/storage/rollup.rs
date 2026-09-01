//! Pre-aggregated 4-minute buckets, so the long timescales don't re-derive
//! a week of raw samples on every load.
//!
//! `query_bucketed` computes 36 aggregates over every row in the window. At
//! `7d` that is 226 000 rows, measured at 1.2 s inside the daemon — the single
//! slowest thing the server does, and it runs on a cold page load. The rows it
//! reads never change: `metrics_raw` is append-only and the bucket grid is
//! absolute (`floor(t / width) * width`), so every completed bucket's answer is
//! already final the moment the bucket closes.
//!
//! So we compute each 4-minute bucket once, when it closes, and read 360 rows
//! instead of 226 000.
//!
//! **The numbers do not change.** A mean of means is only equal to the mean
//! when the groups are the same size, which buckets are not — a gap in
//! collection leaves one short. So each row carries its `sample_count` and the
//! re-aggregation is `SUM(avg * n) / SUM(n)`, which is exactly the mean of the
//! underlying rows. `MIN`/`MAX` compose without weighting. The alignment that
//! makes this work is that every requested bucket width is a whole multiple of
//! [`ROLLUP_BUCKET_MS`] and both grids start from the epoch, so a 4-minute
//! bucket is never split across two output buckets.
//!
//! `tests::rollup_matches_raw` asserts that equality against real rows rather
//! than trusting the argument.

use crate::collector::snapshot::{AggregatedMetric, MetricBand};
use crate::storage::db::MetricsDb;
use crate::sync::guard;
use rusqlite::{params, Connection, Row};

/// Width of one pre-aggregated bucket.
///
/// Every range served from the rollup must use a whole multiple of this (see
/// the module docs). 4 minutes is `24h`'s own bucket width, so that range is a
/// direct read with no re-aggregation at all.
pub const ROLLUP_BUCKET_MS: u64 = 240_000;

/// How far the rollup has been computed, as a Unix-ms bucket boundary.
/// Everything at or after this is still served from `metrics_raw`.
const META_ROLLUP_THROUGH: &str = "metrics_rollup_through_ms";

/// Columns carried as a plain average, in the order `AggregatedMetric` reads
/// them back. Declared once: the create, the fold, the re-aggregation and the
/// raw-tail query are all generated from this list, so they cannot drift.
const AVG_COLS: [&str; 17] = [
    "cpu_total",
    "cpu_p_cores",
    "cpu_e_cores",
    "gpu_usage",
    "mem_used",
    "mem_swap_used",
    "disk_read_bytes_sec",
    "disk_write_bytes_sec",
    "net_up_bytes_sec",
    "net_down_bytes_sec",
    "power_total_w",
    "power_cpu_w",
    "power_gpu_w",
    "power_other_w",
    "cpu_temp_c",
    "gpu_temp_c",
    "fan_rpm",
];

/// Columns that additionally carry the bucket's `[min, max]`. Same set the
/// charts draw a band for — see `MetricBand`.
const BAND_COLS: [&str; 9] = [
    "cpu_total",
    "gpu_usage",
    "mem_used",
    "power_total_w",
    "net_up_bytes_sec",
    "net_down_bytes_sec",
    "disk_read_bytes_sec",
    "disk_write_bytes_sec",
    "cpu_temp_c",
];

/// Averages stored as REAL even where the source column is an integer.
///
/// The weighted re-aggregation multiplies by a sample count, so rounding to an
/// integer per 4-minute bucket first would bias every longer bucket. The cast
/// back to an integer happens once, in the outermost select.
fn create_table_sql() -> String {
    let cols = stored_col_names()
        .iter()
        .map(|c| format!("                {c} REAL"))
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        "CREATE TABLE IF NOT EXISTS metrics_4min (
                timestamp    INTEGER PRIMARY KEY,
                sample_count INTEGER NOT NULL,
{cols}
            );"
    )
}

/// `AVG(x)`, `MIN(x)`, `MAX(x)` over `metrics_raw` — the select list that
/// produces one rollup row, minus the bucket key and count.
fn raw_aggregate_exprs() -> String {
    let mut parts: Vec<String> = AVG_COLS.iter().map(|c| format!("AVG({c})")).collect();
    for c in BAND_COLS {
        parts.push(format!("MIN({c})"));
        parts.push(format!("MAX({c})"));
    }
    parts.join(", ")
}

/// Column names in the same order as [`raw_aggregate_exprs`].
fn stored_col_names() -> Vec<String> {
    let mut names: Vec<String> = AVG_COLS.iter().map(|c| c.to_string()).collect();
    for c in BAND_COLS {
        names.push(format!("{c}_min"));
        names.push(format!("{c}_max"));
    }
    names
}

/// The outermost select: fold 4-minute parts into the requested bucket width.
///
/// Integer-valued series are cast back here and only here, for the reason
/// given on [`create_table_sql`].
fn reaggregate_exprs() -> String {
    const INTEGER_SERIES: [&str; 6] = [
        "mem_used",
        "mem_swap_used",
        "disk_read_bytes_sec",
        "disk_write_bytes_sec",
        "net_up_bytes_sec",
        "net_down_bytes_sec",
    ];
    let mut parts: Vec<String> = AVG_COLS
        .iter()
        .map(|c| {
            // Weighted, not a mean of means — see the module docs.
            let weighted = format!("SUM({c} * n) / SUM(n)");
            if INTEGER_SERIES.contains(c) {
                format!("CAST({weighted} AS INTEGER)")
            } else {
                weighted
            }
        })
        .collect();
    for c in BAND_COLS {
        let min = format!("MIN({c}_min)");
        let max = format!("MAX({c}_max)");
        if INTEGER_SERIES.contains(&c) {
            parts.push(format!("CAST({min} AS INTEGER)"));
            parts.push(format!("CAST({max} AS INTEGER)"));
        } else {
            parts.push(min);
            parts.push(max);
        }
    }
    parts.join(", ")
}

/// Read one output row into an `AggregatedMetric`.
///
/// Column order is fixed by [`AVG_COLS`] then [`BAND_COLS`], both of which
/// generate every query above, so the indices here are the one place that has
/// to agree with that order.
fn decode(row: &Row) -> rusqlite::Result<AggregatedMetric> {
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
        // Rows predating the temperature/fan columns come back NULL.
        cpu_temp_c: row.get::<_, Option<f32>>(15)?.unwrap_or(0.0),
        gpu_temp_c: row.get::<_, Option<f32>>(16)?.unwrap_or(0.0),
        fan_rpm: row.get::<_, Option<f32>>(17)?.unwrap_or(0.0),
        band: MetricBand {
            cpu_total: [row.get(18)?, row.get(19)?],
            gpu_usage: [row.get(20)?, row.get(21)?],
            mem_used: [row.get(22)?, row.get(23)?],
            power_total_w: [row.get(24)?, row.get(25)?],
            net_up_bytes_sec: [row.get(26)?, row.get(27)?],
            net_down_bytes_sec: [row.get(28)?, row.get(29)?],
            disk_read_bytes_sec: [row.get(30)?, row.get(31)?],
            disk_write_bytes_sec: [row.get(32)?, row.get(33)?],
            cpu_temp_c: [
                row.get::<_, Option<f32>>(34)?.unwrap_or(0.0),
                row.get::<_, Option<f32>>(35)?.unwrap_or(0.0),
            ],
        },
    })
}

/// Create the rollup table. Called from `MetricsDb::open`.
pub fn create_schema(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(&create_table_sql())
}

impl MetricsDb {
    /// Whether `bucket_ms` can be served from the rollup.
    ///
    /// A width that is not a whole multiple of [`ROLLUP_BUCKET_MS`] would split
    /// a stored bucket across two output buckets, and neither half's numbers
    /// exist. Such a range falls back to `query_bucketed` over the raw rows,
    /// which is what the short timescales do anyway — at `1h` the window holds
    /// ~1 800 rows and the query costs 8 ms.
    pub fn rollup_serves(bucket_ms: u64) -> bool {
        bucket_ms >= ROLLUP_BUCKET_MS && bucket_ms % ROLLUP_BUCKET_MS == 0
    }

    /// Boundary up to which buckets have been folded. Rows at or after this
    /// are still only in `metrics_raw`.
    pub fn rollup_through(&self) -> u64 {
        self.meta_get(META_ROLLUP_THROUGH)
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    }

    /// Fold every bucket that has closed since the last call.
    ///
    /// `now` bounds the work: the bucket containing it is still filling, and
    /// freezing a partial bucket would leave the chart permanently wrong for
    /// that slot. Returns how many buckets were written.
    ///
    /// Cheap to call often — with nothing to do it aggregates an empty range.
    /// The first call after an upgrade folds the whole retained window, which
    /// is one pass over `metrics_raw` and the only expensive one.
    pub fn roll_up(&self, now: u64) -> Result<usize, rusqlite::Error> {
        let through = self.rollup_through();
        let current_bucket = (now / ROLLUP_BUCKET_MS) * ROLLUP_BUCKET_MS;
        if current_bucket <= through {
            return Ok(0);
        }

        let cols = stored_col_names().join(", ");
        let aggs = raw_aggregate_exprs();
        let sql = format!(
            "INSERT OR REPLACE INTO metrics_4min(timestamp, sample_count, {cols})
             SELECT (timestamp / ?1) * ?1 AS bucket, COUNT(*), {aggs}
             FROM metrics_raw
             WHERE timestamp >= ?2 AND timestamp < ?3
             GROUP BY bucket"
        );

        let written = {
            let conn = guard(self.conn.lock());
            conn.execute(&sql, params![ROLLUP_BUCKET_MS, through, current_bucket])?
        };
        self.meta_set(META_ROLLUP_THROUGH, &current_bucket.to_string());
        Ok(written)
    }

    /// Drop rollup rows older than `before`, mirroring `prune_raw`.
    ///
    /// The rollup is not the long-term archive — it is a cache of what
    /// `metrics_raw` already says, so it retains exactly as long. Keeping it
    /// longer would silently extend history past the point the raw rows can
    /// corroborate it, which is a decision for whoever adds a timescale beyond
    /// the retention window, not a side effect of this cache.
    pub fn prune_rollup(&self, before: u64) -> Result<usize, rusqlite::Error> {
        let conn = guard(self.conn.lock());
        conn.execute(
            "DELETE FROM metrics_4min WHERE timestamp < ?1",
            params![before],
        )
    }

    /// Bucketed history for a width the rollup can serve.
    ///
    /// Reads the folded buckets and unions them with whatever tail of
    /// `metrics_raw` has not been folded yet, then re-aggregates both into
    /// `bucket_ms`. The tail is at most one rollup interval wide, so it is a
    /// few hundred rows regardless of the range asked for.
    ///
    /// `since` is floored to the rollup grid, and the returned window can
    /// therefore start up to [`ROLLUP_BUCKET_MS`] earlier than asked. That is
    /// not a rounding convenience — it is what makes this agree with
    /// `query_bucketed` to the last digit. A raw query filters *rows* by
    /// `since`; a rollup query can only filter whole *buckets*. Given a
    /// `since` that lands mid-bucket, the raw path would fold only the rows
    /// after it while the rollup holds the whole bucket, and the leading point
    /// of the chart would differ depending on which path served it. Flooring
    /// makes both paths see exactly the same rows.
    pub fn query_bucketed_rolled(
        &self,
        since: u64,
        bucket_ms: u64,
    ) -> Result<Vec<AggregatedMetric>, rusqlite::Error> {
        debug_assert!(Self::rollup_serves(bucket_ms));

        let since = (since / ROLLUP_BUCKET_MS) * ROLLUP_BUCKET_MS;
        let through = self.rollup_through();
        let tail_from = since.max(through);
        let stored = stored_col_names().join(", ");
        let aggs = raw_aggregate_exprs();
        let out = reaggregate_exprs();

        let sql = format!(
            "WITH parts AS (
                 SELECT timestamp AS t, sample_count AS n, {stored}
                 FROM metrics_4min
                 WHERE timestamp >= ?2 AND timestamp < ?3
                 UNION ALL
                 SELECT (timestamp / ?4) * ?4 AS t, COUNT(*) AS n, {aggs}
                 FROM metrics_raw
                 WHERE timestamp >= ?5
                 GROUP BY t
             )
             SELECT (t / ?1) * ?1 AS bucket, {out}
             FROM parts
             GROUP BY bucket
             ORDER BY bucket ASC"
        );

        let conn = guard(self.conn.lock());
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![bucket_ms, since, through, ROLLUP_BUCKET_MS, tail_from],
            |row| decode(row),
        )?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A start time on the rollup grid. Off-grid, the first bucket of a
    /// window is partial, which is a real behaviour (covered by
    /// `an_off_grid_since_is_floored`) but noise for the equality tests.
    const ALIGNED_BASE: u64 = (1_700_000_000_000 / ROLLUP_BUCKET_MS) * ROLLUP_BUCKET_MS;

    fn mk_db() -> MetricsDb {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = format!("/tmp/webtop-rollup-test-{}-{n}.db", std::process::id());
        let _ = std::fs::remove_file(&path);
        MetricsDb::open(&path).expect("open test db")
    }

    /// Write rows at a deliberately uneven cadence, with a gap, so a mean of
    /// means would differ from the true mean and the weighting is actually
    /// exercised.
    fn seed(db: &MetricsDb, base: u64) {
        let mut snap = crate::collector::snapshot::SystemSnapshot::default();
        let mut t = base;
        let mut i = 0u64;
        while t < base + 40 * ROLLUP_BUCKET_MS {
            snap.timestamp = t;
            snap.cpu_total = (i % 97) as f32;
            snap.gpu_usage = (i % 13) as f32;
            snap.mem_used = 1_000_000_000 + (i % 31) * 7_000_000;
            snap.power_total_w = 10.0 + (i % 7) as f32;
            snap.cpu_temp_c = 40.0 + (i % 11) as f32;
            db.insert_raw(&snap).expect("insert");
            i += 1;
            // Uneven spacing, and a long gap every so often, so buckets end up
            // holding different numbers of samples.
            t += if i % 17 == 0 { 91_000 } else { 2_000 };
        }
    }

    #[test]
    fn rollup_matches_raw() {
        let db = mk_db();
        let base = ALIGNED_BASE;
        seed(&db, base);
        let now = base + 40 * ROLLUP_BUCKET_MS;

        let written = db.roll_up(now).expect("roll up");
        assert!(written > 0, "expected folded buckets, got {written}");

        for bucket_ms in [
            ROLLUP_BUCKET_MS,
            ROLLUP_BUCKET_MS * 7,
            ROLLUP_BUCKET_MS * 15,
        ] {
            let raw = db.query_bucketed(base, bucket_ms).expect("raw");
            let rolled = db.query_bucketed_rolled(base, bucket_ms).expect("rolled");
            assert_eq!(
                raw.len(),
                rolled.len(),
                "bucket count differs at width {bucket_ms}"
            );
            for (r, o) in raw.iter().zip(rolled.iter()) {
                assert_eq!(r.timestamp, o.timestamp);
                assert!(
                    (r.cpu_total - o.cpu_total).abs() < 1e-3,
                    "cpu mean differs at {} width {bucket_ms}: {} vs {}",
                    r.timestamp,
                    r.cpu_total,
                    o.cpu_total
                );
                assert_eq!(
                    r.mem_used, o.mem_used,
                    "mem mean differs at {}",
                    r.timestamp
                );
                assert_eq!(
                    r.band.cpu_total, o.band.cpu_total,
                    "cpu band differs at {}",
                    r.timestamp
                );
                assert_eq!(
                    r.band.mem_used, o.band.mem_used,
                    "mem band differs at {}",
                    r.timestamp
                );
            }
        }
    }

    #[test]
    fn the_still_filling_bucket_is_never_frozen() {
        let db = mk_db();
        let base = ALIGNED_BASE;
        seed(&db, base);
        // `now` sits inside the bucket that starts here.
        let open_bucket = base + 10 * ROLLUP_BUCKET_MS;
        db.roll_up(open_bucket + 1000).expect("roll up");
        assert_eq!(
            db.rollup_through(),
            open_bucket,
            "the bucket containing `now` must stay out of the rollup"
        );
    }

    #[test]
    fn the_unfolded_tail_still_reaches_the_reader() {
        let db = mk_db();
        let base = ALIGNED_BASE;
        seed(&db, base);

        // Fold only the first half, leaving a long tail in `metrics_raw`.
        db.roll_up(base + 20 * ROLLUP_BUCKET_MS).expect("partial");

        let raw = db.query_bucketed(base, ROLLUP_BUCKET_MS * 7).expect("raw");
        let rolled = db
            .query_bucketed_rolled(base, ROLLUP_BUCKET_MS * 7)
            .expect("rolled");
        assert_eq!(raw.len(), rolled.len(), "half-folded window lost buckets");
        for (r, o) in raw.iter().zip(rolled.iter()) {
            assert_eq!(r.timestamp, o.timestamp);
            assert!((r.cpu_total - o.cpu_total).abs() < 1e-3);
            assert_eq!(r.band.cpu_total, o.band.cpu_total);
        }
    }

    #[test]
    fn an_off_grid_since_is_floored() {
        let db = mk_db();
        let base = ALIGNED_BASE;
        seed(&db, base);
        db.roll_up(base + 40 * ROLLUP_BUCKET_MS).expect("roll up");

        // Ask from a point one minute into a bucket. The window must start at
        // that bucket's boundary, not inside it — a partial leading bucket
        // would read differently depending on which path served it.
        let off_grid = base + 10 * ROLLUP_BUCKET_MS + 60_000;
        let rolled = db
            .query_bucketed_rolled(off_grid, ROLLUP_BUCKET_MS)
            .expect("rolled");
        assert_eq!(
            rolled.first().map(|r| r.timestamp),
            Some(base + 10 * ROLLUP_BUCKET_MS),
            "an off-grid `since` must floor to the bucket containing it"
        );
        // And it still equals the raw answer over that floored window.
        let raw = db
            .query_bucketed(base + 10 * ROLLUP_BUCKET_MS, ROLLUP_BUCKET_MS)
            .expect("raw");
        assert_eq!(raw.len(), rolled.len());
        for (r, o) in raw.iter().zip(rolled.iter()) {
            assert_eq!(r.timestamp, o.timestamp);
            assert!((r.cpu_total - o.cpu_total).abs() < 1e-3);
        }
    }

    #[test]
    fn only_whole_multiples_are_served_from_the_rollup() {
        assert!(MetricsDb::rollup_serves(ROLLUP_BUCKET_MS));
        assert!(MetricsDb::rollup_serves(ROLLUP_BUCKET_MS * 7));
        assert!(!MetricsDb::rollup_serves(ROLLUP_BUCKET_MS / 2));
        // 30 minutes is not a multiple of 4 — the width `7d` used to use.
        assert!(!MetricsDb::rollup_serves(1_800_000));
    }
}
