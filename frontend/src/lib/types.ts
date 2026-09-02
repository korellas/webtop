export interface SystemSnapshot {
  timestamp: number;
  cpu_total: number;
  cpu_p_cores: number;
  cpu_e_cores: number;
  /** Per-core utilisation 0..100. Empty on aggregated history rows. */
  cpu_cores?: number[];
  gpu_usage: number;
  mem_used: number;
  mem_total: number;
  mem_swap_used: number;
  /** VM breakdown; all zeros on aggregated rows. */
  mem_breakdown?: MemBreakdown;
  disk_read_bytes_sec: number;
  disk_write_bytes_sec: number;
  disk_used: number;
  disk_total: number;
  net_up_bytes_sec: number;
  net_down_bytes_sec: number;
  power_total_w: number;
  power_cpu_w: number;
  power_gpu_w: number;
  power_other_w: number;
  /** CPU die °C — 0 on Intel Macs / when sensors aren't readable. */
  cpu_temp_c?: number;
  /** GPU die °C — 0 on Intel Macs / when sensors aren't readable. */
  gpu_temp_c?: number;
  /** Highest fan RPM across all fans — 0 on fanless Macs. */
  fan_rpm?: number;
  energy_session_wh: number;
  energy_prev_month_wh: number;
  /** Battery info — null on desktop Macs. */
  battery?: BatteryInfo | null;
  processes: ProcessInfo[];
  /**
   * Per-bucket `[min, max]` for the charted series. Present only on aggregated
   * history rows; live samples omit it because a single point reading is
   * already its own extreme. See `attachBands` in `chart-utils`.
   */
  band?: MetricBand;
}

/** `[min, max]` over the rows the backend folded into one history bucket. */
export interface MetricBand {
  cpu_total: [number, number];
  gpu_usage: [number, number];
  mem_used: [number, number];
  power_total_w: [number, number];
  net_up_bytes_sec: [number, number];
  net_down_bytes_sec: [number, number];
  disk_read_bytes_sec: [number, number];
  disk_write_bytes_sec: [number, number];
  cpu_temp_c: [number, number];
}

export interface ProcessInfo {
  pid: number;
  name: string;
  /** Owning user (e.g. "alice", "root"). Optional for history rows. */
  user?: string;
  cpu_percent: number;
  gpu_percent: number;
  mem_bytes: number;
  /** Full command line, truncated server-side. Empty on older rows. */
  cmd?: string;
}

// ─── Services panel ─────────────────────────────────────────────────────────

/**
 * How a declared service is doing.
 *
 * `unhealthy` is the state worth reacting to: the process is alive, so launchd
 * is satisfied and nothing else on the machine will complain, but the port has
 * been closed longer than any legitimate startup takes.
 */
export type ServiceState =
  | 'running'
  | 'starting'
  | 'unhealthy'
  | 'down'
  | 'unregistered'
  | 'rogue';

export interface ServiceStatus {
  name: string;
  label: string;
  port: number | null;
  group: string;
  /** Declared memory ceiling in bytes, or null when the service has none. */
  mem_budget: number | null;
  depends_on: string[];

  state: ServiceState;
  /** launchd's PID — the root of the tree, often a wrapper shell. */
  pid: number | null;
  /** Last exit status launchd recorded. Negative values are signal numbers. */
  last_exit: number | null;
  port_open: boolean;
  /** Summed over the service's entire process tree, not just its root. */
  mem_bytes: number;
  cpu_percent: number;
  proc_count: number;
  uptime_sec: number;
}

export interface ServicesResponse {
  services: ServiceStatus[];
  manifest_path: string;
  /** Set when the manifest is missing or malformed; the panel shows it
   *  instead of an unexplained empty list. */
  error: string | null;
}

export interface MemBreakdown {
  wired: number;
  active: number;
  inactive: number;
  compressed: number;
  free: number;
}

export interface BatteryInfo {
  percent: number;
  is_charging: boolean;
  is_plugged_in: boolean;
  time_remaining_sec: number | null;
  cycle_count: number | null;
  health_percent: number | null;
  charge_rate_w: number | null;
}

export interface SystemInfo {
  model: string;
  chip: string;
  p_core_count: number;
  e_core_count: number;
  gpu_core_count: number;
  mem_total: number;
  disk_total: number;
  os_version: string;
  /** Detected link speed in bytes/sec; undefined on older backends (default 125 MB/s). */
  net_link_speed_bytes_sec?: number;
  /** Per-logical-core kind ("P" | "E"). Order matches `SystemSnapshot.cpu_cores`. */
  core_kinds?: string[];
}

// ─── On-demand endpoint payloads ────────────────────────────────────────────

export interface DiskInfo {
  name: string;
  mount_point: string;
  fs_type: string;
  total_bytes: number;
  used_bytes: number;
  is_removable: boolean;
  is_boot: boolean;
}

/** One directory in the disk drawer's largest-folders tree. */
export interface FolderRow {
  path: string;
  name: string;
  size_bytes: number;
  file_count: number;
  /** Unix ms this row's size was last measured. Per-row, not per-scan. */
  scanned_at: number;
  /** Sub-entries that could not be read; non-zero means size is a lower bound. */
  unreadable: number;
  has_children: boolean;
}

export interface FoldersResponse {
  path: string;
  total: FolderRow | null;
  children: FolderRow[];
  last_full_scan_at: number | null;
  scanning: boolean;
  never_scanned: boolean;
}

export interface VerifyResponse {
  updated: FolderRow[];
  ran: boolean;
}

export interface WirelessInfo {
  ssid: string | null;
  bssid: string | null;
  channel: number | null;
  band: string | null;
  rssi_dbm: number | null;
  noise_dbm: number | null;
  tx_rate_mbps: number | null;
  security: string | null;
}

export interface NetInterfaceInfo {
  name: string;
  is_up: boolean;
  mac: string | null;
  ipv4: string[];
  ipv6: string[];
  link_speed_bps: number | null;
  rx_bytes_sec: number;
  tx_bytes_sec: number;
  mtu: number | null;
  kind: 'wifi' | 'ethernet' | 'bridge' | 'vpn' | 'loopback' | 'p2p' | 'other';
  wireless?: WirelessInfo | null;
}

/**
 * Total bytes transferred over the currently selected timescale — the
 * network chart's rate curve integrated over time server-side, not derived
 * from the (downsampled) chart data. See `MetricsDb::net_totals`.
 */
export interface NetworkTotals {
  up_bytes: number;
  down_bytes: number;
}

/** One bucket of `/api/network_history` — mirrors `EnergyBucket`, but with
 *  both directions since a byte total has one where energy doesn't. */
export interface NetworkHistoryBucket {
  bucket_start: number;
  up_bytes: number;
  down_bytes: number;
}

export interface NetworkHistory {
  group: 'hour' | 'day' | 'week' | 'month' | string;
  buckets: NetworkHistoryBucket[];
  total_up_bytes: number;
  total_down_bytes: number;
}

export interface EnergyBucket {
  bucket_start: number;
  wh: number;
}

export interface EnergyHistory {
  group: 'hour' | 'day' | 'week' | 'month' | string;
  buckets: EnergyBucket[];
  total_wh: number;
  avg_wh: number;
  peak_wh: number;
}

/**
 * Selectable history windows.
 *
 * Capped at 7d because that is all the data there is: `metrics_raw` is pruned
 * to 7 days and every timescale is derived from it at query time. A `30d`
 * option used to live here, but the backend has no `30d` arm — it returned
 * 400, `loadHistory` swallowed the error, and the dashboard was left with an
 * empty store plotted on a 30-day axis. Because the selection persists to
 * localStorage, that broken state survived reloads until the user happened to
 * click a different pill. Do not re-add a window longer than the retention
 * period without also adding a long-term rollup table to back it.
 */
export type Timescale = '1m' | '5m' | '15m' | '1h' | '24h' | '7d';
