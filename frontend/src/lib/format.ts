export function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  const val = bytes / Math.pow(1024, i);
  return `${val < 10 ? val.toFixed(1) : Math.round(val)} ${units[i]}`;
}

export function formatBytesPerSec(bytes: number): string {
  return `${formatBytes(bytes)}/s`;
}

/** Bytes → GB with a fixed number of decimals (always GB, no auto-scaling) */
export function formatGB(bytes: number, decimals: number = 1): string {
  const gb = bytes / (1024 ** 3);
  return `${gb.toFixed(decimals)} GB`;
}

export function formatPercent(value: number): string {
  return `${value < 10 ? value.toFixed(1) : Math.round(value)}%`;
}

export function formatWatts(watts: number): string {
  return `${watts < 10 ? watts.toFixed(1) : Math.round(watts)}W`;
}

/**
 * Format cumulative energy. Always in Wh (we reset monthly so kWh is rarely needed).
 * Falls through to kWh only when the value is huge.
 */
export function formatWh(wh: number): string {
  if (!isFinite(wh) || wh < 0) return '0 Wh';
  if (wh < 10) return `${wh.toFixed(2)} Wh`;
  if (wh < 1000) return `${wh.toFixed(1)} Wh`;
  if (wh < 10_000) return `${wh.toFixed(0)} Wh`;
  return `${(wh / 1000).toFixed(2)} kWh`;
}

// Legacy alias retained to avoid mass-renaming imports.
export const formatKwh = formatWh;

/** Bytes/sec → Mbps (megabits per second) */
export function formatMbps(bytesPerSec: number): string {
  const mbps = (bytesPerSec * 8) / 1_000_000;
  if (mbps < 0.01) return '0 Mbps';
  if (mbps < 1) return `${(mbps * 1000).toFixed(0)} Kbps`;
  if (mbps < 100) return `${mbps.toFixed(1)} Mbps`;
  return `${Math.round(mbps)} Mbps`;
}

/**
 * Bytes/sec → MB/s with a fixed 4-significant-digit format so the decimal
 * point shifts smoothly as magnitude grows rather than the whole string
 * jumping around. Digit count stays ≤ 5 chars so tabular-nums stays stable.
 *
 *   0.030  0.300  1.300  11.30  123.0  1234  12345
 */
export function formatMBps(bytesPerSec: number): string {
  const mb = bytesPerSec / (1024 * 1024);
  if (!isFinite(mb) || mb <= 0) return '0.000 MB/s';
  if (mb < 10)   return `${mb.toFixed(3)} MB/s`;
  if (mb < 100)  return `${mb.toFixed(2)} MB/s`;
  if (mb < 1000) return `${mb.toFixed(1)} MB/s`;
  return `${Math.round(mb)} MB/s`;
}

/**
 * Compact "how long ago" label for per-row freshness in the folder tree.
 *
 * Anything inside a few seconds reads as "now" — the on-open verification
 * finishes in that window, and "3s ago" invites the reader to wonder whether
 * something is stale when nothing is.
 */
export function formatAgo(timestampMs: number, nowMs: number = Date.now()): string {
  const seconds = Math.max(0, Math.round((nowMs - timestampMs) / 1000));
  if (seconds < 10) return 'now';
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}
