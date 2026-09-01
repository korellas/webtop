import type { MetricBand, SystemSnapshot, Timescale } from './types';

/**
 * Suffixes marking the lower/upper bound of a series' per-bucket range.
 *
 * `downsample` keys its aggregation rule off these, so a series gains a band
 * purely by emitting `<key>_lo` / `<key>_hi` alongside `<key>` — no registry
 * of "which metrics have bands" to keep in sync.
 */
export const BAND_LO = '_lo';
export const BAND_HI = '_hi';

/**
 * Series the backend ships min/max for. Every one of these is also a numeric
 * field on `SystemSnapshot`, which is what makes the live-sample fallback in
 * `attachBands` well defined.
 */
const BANDED: ReadonlyArray<keyof MetricBand> = [
  'cpu_total',
  'gpu_usage',
  'mem_used',
  'power_total_w',
  'net_up_bytes_sec',
  'net_down_bytes_sec',
  'disk_read_bytes_sec',
  'disk_write_bytes_sec',
  'cpu_temp_c',
];

/**
 * Flatten `snapshot.band` into `<key>_lo` / `<key>_hi` numeric fields.
 *
 * Live WebSocket samples carry no `band` — a 1-second reading is its own
 * extreme — so both bounds fall back to the point value. That makes the band
 * collapse onto the mean line across the live tail instead of vanishing, which
 * would otherwise leave a visible seam where history meets real time.
 */
export function attachBands(rows: SystemSnapshot[]): Record<string, unknown>[] {
  return rows.map((r) => {
    const out: Record<string, unknown> = { ...r };
    const flat = r as unknown as Record<string, unknown>;
    for (const key of BANDED) {
      const pair = r.band?.[key];
      const raw = flat[key];
      const point = typeof raw === 'number' ? raw : 0;
      out[`${key}${BAND_LO}`] = pair ? pair[0] : point;
      out[`${key}${BAND_HI}`] = pair ? pair[1] : point;
    }
    // The nested object has served its purpose; dropping it keeps every
    // remaining value scalar, which is what `downsample` assumes.
    delete out.band;
    return out;
  });
}

/** Duration in ms for each timescale */
export const TIMESCALE_MS: Record<Timescale, number> = {
  '1m': 60_000,
  '5m': 300_000,
  '15m': 900_000,
  '1h': 3_600_000,
  '24h': 86_400_000,
  '7d': 604_800_000,
};

/**
 * Max data points to render per timescale.
 *
 * These MUST mirror the server's bucket widths in `api.rs::get_history`, or the
 * client re-buckets history on a different grid than the server used and the
 * two disagree about where each point sits.
 *
 * Sized against the collector's real ~2 s cadence (measured 2026-08-01: 29 rows
 * in 60 s), not the 1 s the comments used to claim. A bucket narrower than
 * ~2× the sample period holds one row, so nothing is actually averaged — that
 * is why the 5 m view looked like raw noise rather than a five-minute overview.
 */
const MAX_POINTS: Record<Timescale, number> = {
  '1m': 30,    // 2s buckets — raw by design, only ~30 samples exist in the window
  '5m': 75,    // 4s buckets
  '15m': 150,  // 6s buckets
  '1h': 360,   // 10s buckets
  '24h': 360,  // 4min buckets
  '7d': 360,   // 28min buckets — a whole multiple of the server's 4min rollup grid
};

/**
 * Downsample a time series to at most MAX_POINTS[timescale] buckets using
 * **absolute-time bucketing** — every sample's bucket index is computed as
 * `floor(timestamp / bucketMs)`, a number that doesn't depend on when we
 * query. That means:
 *
 *   • Each historical bucket is immutable. Once its time range has passed,
 *     its average is frozen — new samples can never retroactively shift it.
 *   • Only the single bucket currently being filled (the one containing
 *     `now`) updates as live samples stream in, and once we cross into the
 *     next bucket it joins the frozen tail.
 *
 * This kills the "values jiggle every tick" effect that a sliding window
 * (bucket grid relative to `now`) produces: with relative alignment, every
 * sample crosses bucket boundaries as time advances, so every bucket's
 * average drifts even when nothing about the underlying measurement changes.
 *
 * Empty buckets are skipped (Recharts connects surviving points with a line).
 */
export function downsample<T extends { timestamp: number }>(
  data: T[],
  timescale: Timescale,
): T[] {
  if (data.length === 0) return data;

  const max = MAX_POINTS[timescale];
  const windowMs = TIMESCALE_MS[timescale];
  const bucketMs = Math.max(1000, Math.floor(windowMs / max));

  // Fast-path: already at or below bucket resolution (e.g. 1m view with
  // raw 1-s samples, bucketMs == 1000 and data.length <= 60).
  if (data.length <= max && bucketMs <= 1500) return data;

  // Absolute bucket index for a sample at time T: `floor(T / bucketMs)`.
  // This is stable across calls — the same sample always maps to the same
  // bucket, so a bucket's membership (and therefore its average) never
  // changes unless new samples actually land inside that time range.
  const latestTs = data[data.length - 1].timestamp;
  const latestBucket = Math.floor(latestTs / bucketMs);
  const oldestBucket = latestBucket - max + 1;

  const buckets = new Map<number, T[]>();
  for (const row of data) {
    const idx = Math.floor(row.timestamp / bucketMs);
    if (idx < oldestBucket || idx > latestBucket) continue;
    const arr = buckets.get(idx);
    if (arr) arr.push(row);
    else buckets.set(idx, [row]);
  }

  // Cache numeric vs non-numeric keys once so the hot loop is tight.
  const probe = data[0] as Record<string, unknown>;
  const numericKeys: string[] = [];
  const nonNumericKeys: string[] = [];
  for (const k of Object.keys(probe)) {
    if (typeof probe[k] === 'number') numericKeys.push(k);
    else nonNumericKeys.push(k);
  }

  /**
   * Fold one bucket's rows into a single row.
   *
   * Band bounds must NOT be averaged. Taking the mean of a bucket's `_hi`
   * values is how a peak gets erased a second time: the backend already
   * preserved the maximum per SQL bucket, and averaging those maxima here
   * would throw that away again — exactly the flattening the band exists to
   * prevent. `_lo` folds with `Math.min`, `_hi` with `Math.max`, everything
   * else with the mean.
   */
  function foldBucket(bucket: T[]): Record<string, unknown> {
    const agg: Record<string, unknown> = {};
    for (const k of numericKeys) {
      const mode = k.endsWith(BAND_LO) ? 'min' : k.endsWith(BAND_HI) ? 'max' : 'mean';
      let sum = 0;
      let count = 0;
      let extreme = mode === 'min' ? Infinity : -Infinity;
      for (const row of bucket) {
        const v = (row as Record<string, unknown>)[k];
        if (typeof v !== 'number' || !Number.isFinite(v)) continue;
        count++;
        if (mode === 'mean') sum += v;
        else if (mode === 'min') extreme = Math.min(extreme, v);
        else extreme = Math.max(extreme, v);
      }
      if (count > 0) agg[k] = mode === 'mean' ? sum / count : extreme;
    }
    return agg;
  }

  const sortedIdx = Array.from(buckets.keys()).sort((a, b) => a - b);
  // The last bucket (still filling) is treated specially: we pass the latest
  // sample through unchanged so a sudden jump (e.g. CPU 0→100%) is visible
  // on the chart immediately, not averaged down by the first few samples in
  // the bucket. Every earlier bucket keeps its mean for a smooth trend line.
  const currentBucketIdx = sortedIdx.length > 0 ? sortedIdx[sortedIdx.length - 1] : null;

  const result: T[] = [];
  for (const idx of sortedIdx) {
    const bucket = buckets.get(idx)!;
    const bucketCenter = idx * bucketMs + bucketMs / 2;

    // Real-time edge: show the *running mean* of the still-filling bucket,
    // not the raw latest sample. Passing the raw value through made the
    // right-hand tail snap to each new 1-s reading every tick while the rest
    // of the line stayed smoothed — a visible "jumping seam". Averaging the
    // bucket-so-far makes the tail glide and converge instead. We keep the
    // live sample's real timestamp so the edge still sits exactly at "now".
    if (idx === currentBucketIdx) {
      const liveSample = bucket[bucket.length - 1] as Record<string, unknown>;
      if (bucket.length === 1) {
        result.push({ ...liveSample } as T);
        continue;
      }
      const agg = foldBucket(bucket);
      for (const k of nonNumericKeys) {
        agg[k] = liveSample[k];
      }
      agg.timestamp = liveSample.timestamp;
      result.push(agg as T);
      continue;
    }

    if (bucket.length === 1) {
      const row = { ...(bucket[0] as object) } as Record<string, unknown>;
      row.timestamp = bucketCenter;
      result.push(row as T);
      continue;
    }

    const last = bucket[bucket.length - 1] as Record<string, unknown>;
    const agg = foldBucket(bucket);
    for (const k of nonNumericKeys) {
      agg[k] = last[k];
    }
    agg.timestamp = bucketCenter;
    result.push(agg as T);
  }

  return result;
}

/**
 * How many raw collector samples a bucket at this timescale holds.
 *
 * Derived from the widths rather than listed, so changing `MAX_POINTS` cannot
 * leave the callers stale.
 */
const COLLECTOR_PERIOD_MS = 2_000;
const MIN_SAMPLES_PER_BUCKET_FOR_M4 = 5;

function samplesPerBucket(timescale: Timescale): number {
  return TIMESCALE_MS[timescale] / MAX_POINTS[timescale] / COLLECTOR_PERIOD_MS;
}

/**
 * Whether buckets at this timescale aggregate enough to hide anything.
 *
 * At 1 m a bucket is one sample wide, so `min == max == avg` and there is
 * nothing an extremes-preserving pass could recover; at 5 m it is two. Below
 * the threshold the plotted line already *is* the raw data, and `MinMaxTicks`
 * draws nothing.
 */
export function aggregates(timescale: Timescale): boolean {
  return samplesPerBucket(timescale) >= MIN_SAMPLES_PER_BUCKET_FOR_M4;
}

/**
 * Centered rolling mean over the trend series. Per-bucket bounds are left
 * strictly alone, and that is not an oversight — it has been got wrong twice.
 *
 * Averaging the bounds shaves exactly the peaks they exist to preserve, undoing
 * `downsample`'s min/max folding one stage later.
 *
 * Folding them with a *rolling* min/max is worse. A rolling max is a
 * morphological dilation: one 2-second spike becomes a plateau as wide as the
 * window, so what should hug the line spreads into a skyline of rectangles.
 * That version existed to stop a per-bucket band alternating between
 * zero-width and open at 5 m; `aggregates()` now heads that off at the source
 * by not drawing extremes at all where a bucket holds fewer than five samples.
 *
 * The bounds reach the chart through `expandMinMax`, which needs them exactly
 * as the backend measured them.
 */
export function smooth<T extends { timestamp: number }>(
  data: T[],
  window: number,
): T[] {
  if (data.length < 2 || window < 2) return data;
  const half = Math.floor(window / 2);

  // Pre-compute keys once so we don't hammer typeof inside the hot loop.
  const sample = data[Math.floor(data.length / 2)] as Record<string, unknown>;
  const meanKeys: string[] = [];
  for (const k of Object.keys(sample)) {
    if (typeof sample[k] !== 'number') continue;
    if (k.endsWith(BAND_LO) || k.endsWith(BAND_HI)) continue;
    meanKeys.push(k);
  }
  if (meanKeys.length === 0) return data;

  const result: T[] = new Array(data.length);
  // The last `half` points pass through unchanged so live spikes ride through
  // to the chart's right edge without being diluted by a one-sided window. A
  // centered rolling mean at the tail necessarily becomes asymmetric (there's
  // no "future" to average with), which pulls the latest value back toward the
  // past mean — exactly the "why doesn't my 100% spike show?" complaint.
  const edgeFreezeFrom = Math.max(0, data.length - half);

  for (let i = 0; i < data.length; i++) {
    const start = Math.max(0, i - half);
    const end = Math.min(data.length, i + half + 1);
    const agg: Record<string, unknown> = { ...(data[i] as object) };

    if (i < edgeFreezeFrom) {
      for (const key of meanKeys) {
        let sum = 0;
        let count = 0;
        for (let j = start; j < end; j++) {
          const v = (data[j] as Record<string, unknown>)[key];
          if (typeof v === 'number' && Number.isFinite(v)) {
            sum += v;
            count++;
          }
        }
        if (count > 0) agg[key] = sum / count;
      }
    }

    result[i] = agg as T;
  }

  return result;
}

/** X-axis tick formatter based on timescale */
export function formatXTick(ts: number, timescale: Timescale): string {
  const now = Date.now();
  const diff = now - ts;
  if (diff < 0) return '';

  switch (timescale) {
    case '1m':
      return `-${Math.round(diff / 1000)}s`;
    case '5m':
    case '15m':
      return `-${(diff / 60000).toFixed(1)}m`;
    case '1h':
      return `-${Math.round(diff / 60000)}m`;
    case '24h':
      return `-${(diff / 3600000).toFixed(1)}h`;
    case '7d':
      return `-${(diff / 86400000).toFixed(1)}d`;
  }
}

/** Generate fixed X-axis domain [now - duration, now] */
export function getXDomain(timescale: Timescale): [number, number] {
  const now = Date.now();
  return [now - TIMESCALE_MS[timescale], now];
}

/** Tick count hint for X-axis */
export function getXTickCount(timescale: Timescale): number {
  switch (timescale) {
    case '1m': return 4;
    case '5m': return 5;
    case '15m': return 5;
    case '1h': return 4;
    case '24h': return 6;
    case '7d': return 7;
  }
}
