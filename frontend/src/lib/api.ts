import type {
  DiskInfo,
  EnergyHistory,
  FoldersResponse,
  NetInterfaceInfo,
  NetworkHistory,
  NetworkTotals,
  ProcessInfo,
  ServicesResponse,
  Timescale,
  VerifyResponse,
} from './types';

/**
 * Lightweight fetch helper with AbortController support. Each detail drawer
 * passes a signal so we can cancel in-flight requests when the drawer closes.
 */
async function getJson<T>(path: string, signal?: AbortSignal): Promise<T> {
  const res = await fetch(path, { signal });
  if (!res.ok) {
    throw new Error(`${path} → HTTP ${res.status}`);
  }
  return res.json() as Promise<T>;
}

export const fetchDisks = (signal?: AbortSignal) =>
  getJson<DiskInfo[]>('/api/disks', signal);

export const fetchNetworkInterfaces = (signal?: AbortSignal) =>
  getJson<NetInterfaceInfo[]>('/api/network_interfaces', signal);

export const fetchGpuProcesses = (signal?: AbortSignal) =>
  getJson<ProcessInfo[]>('/api/gpu_processes', signal);

/** Total up/down bytes transferred over `range`, integrated server-side. */
export const fetchNetworkTotals = (range: Timescale, signal?: AbortSignal) =>
  getJson<NetworkTotals>(`/api/network_totals?range=${range}`, signal);

/** Up/down byte totals bucketed by hour/day/week/month — mirrors `fetchEnergyHistory`. */
export const fetchNetworkHistory = (
  group: 'hour' | 'day' | 'week' | 'month',
  signal?: AbortSignal,
) => {
  const tz = new Date().getTimezoneOffset();
  return getJson<NetworkHistory>(
    `/api/network_history?group=${group}&tz_offset_minutes=${tz}`,
    signal,
  );
};

export const fetchEnergyHistory = (
  group: 'hour' | 'day' | 'week' | 'month',
  signal?: AbortSignal,
) => {
  // Send the browser's UTC offset so "day" buckets align on *local* midnight
  // (not UTC midnight, which in KST would chop the day at 09:00).
  const tz = new Date().getTimezoneOffset();
  return getJson<EnergyHistory>(
    `/api/energy_history?group=${group}&tz_offset_minutes=${tz}`,
    signal,
  );
};

/**
 * Sent on every control request. The value carries no meaning — its presence
 * is the point: a custom header forces a CORS preflight, webtop answers none,
 * so a page from another origin cannot reach these endpoints even though a
 * plain cross-origin POST would otherwise go through and take effect.
 *
 * Not authentication. Anything on the LAN can set a header; this stops
 * drive-by requests from a browser and nothing more.
 */
const CONTROL_HEADER = { 'X-Svc-Control': '1' };

async function postJson<T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> {
  const res = await fetch(path, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', ...CONTROL_HEADER },
    body: JSON.stringify(body),
    signal,
  });
  if (!res.ok) {
    throw new Error(`${path} → HTTP ${res.status}`);
  }
  return res.json() as Promise<T>;
}

/** Cached folder sizes for `path` (defaults to the scan root). Always instant. */
export const fetchFolders = (path?: string, signal?: AbortSignal) =>
  getJson<FoldersResponse>(
    path ? `/api/folders?path=${encodeURIComponent(path)}` : '/api/folders',
    signal,
  );

/**
 * Re-measure the given folders, cheapest first, within the server's 3s budget.
 * Paths that don't fit are simply absent from the response and keep their
 * cached value — the caller patches in whatever came back.
 */
export const verifyFolders = (paths: string[], signal?: AbortSignal) =>
  postJson<VerifyResponse>('/api/folders/verify', { paths }, signal);

/** Kick off a full background re-walk. Returns immediately. */
export const rescanFolders = (signal?: AbortSignal) =>
  postJson<{ started: boolean }>('/api/folders/rescan', {}, signal);

/** Declared services plus their currently measured state. */
export const fetchServices = (signal?: AbortSignal) =>
  getJson<ServicesResponse>('/api/services', signal);

/**
 * Ask a service to restart (SIGTERM to its process tree root; launchd's
 * KeepAlive brings it back).
 *
 * Resolves with `ok: false` and a human-readable reason for expected failures
 * — unknown name, process already gone — rather than rejecting. Those are
 * answers to show the user, not exceptions to handle.
 */
export const restartService = (name: string, signal?: AbortSignal) =>
  postJson<{ ok: boolean; message: string }>(
    `/api/services/${encodeURIComponent(name)}/restart`,
    {},
    signal,
  );

/** The verbs the control helper accepts. */
export type ServiceVerb = 'start' | 'stop' | 'restart' | 'enable' | 'disable';

/**
 * Run a control verb against a service.
 *
 * webtop performs none of this itself — the request is delegated to a
 * root-owned helper that decides whether the target may be touched at all.
 * Expected refusals ("not in the inventory", "no sudoers rule") come back as
 * `ok: false` with the reason, which is something to show, not to throw on.
 */
export const controlService = (name: string, verb: ServiceVerb, signal?: AbortSignal) =>
  postJson<{ ok: boolean; message: string }>(
    `/api/services/${encodeURIComponent(name)}/control/${verb}`,
    {},
    signal,
  );
