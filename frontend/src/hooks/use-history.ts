import { useEffect } from 'react';
import { fetchNetworkTotals } from '../lib/api';
import { useMetricsStore } from '../store/metrics-store';
import { useTimescaleStore } from '../store/timescale-store';
import { useSystemStore } from '../store/system-store';
import type { SystemInfo, SystemSnapshot, Timescale } from '../lib/types';

/**
 * Fetch the history window for `timescale` and replace the buffer in one shot.
 *
 * This is what makes the dashboard "snap to now": rather than letting the
 * chart animate through a backlog of frames (the old life-flashing-before-
 * your-eyes replay when you returned to a backgrounded tab), we drop the
 * whole window in at once and let the live WebSocket carry on from the edge.
 */
/**
 * The history load currently in flight, if any.
 *
 * Several independent triggers legitimately want to "snap the window to now" —
 * mount, timescale change, WebSocket open, tab-return — and on a cold start
 * two of them fire within milliseconds of each other. That fetched the same
 * 340 kB window twice, and at `7d` it also ran the same second-long `GROUP BY`
 * on the server twice. Collapsing concurrent requests for the same timescale
 * keeps every trigger's intent while paying for the window once; a later
 * reload is not affected, because by then nothing is in flight.
 */
let inFlight: { timescale: Timescale; promise: Promise<void> } | null = null;

export function loadHistory(timescale: Timescale): Promise<void> {
  if (inFlight && inFlight.timescale === timescale) return inFlight.promise;
  const promise = fetchHistory(timescale).finally(() => {
    if (inFlight?.promise === promise) inFlight = null;
  });
  inFlight = { timescale, promise };
  return promise;
}

async function fetchHistory(timescale: Timescale): Promise<void> {
  try {
    const res = await fetch(`/api/history?range=${timescale}`);
    if (!res.ok) throw new Error(`/api/history → HTTP ${res.status}`);
    const data: SystemSnapshot[] = await res.json();
    useMetricsStore.getState().setHistory(data);
  } catch (e) {
    // Record the failure, don't just log it. Swallowing this left the previous
    // window's data on screen with nothing to say it was stale — the exact
    // shape of the old `30d` bug, where an unsupported range 400'd on every
    // load and the dashboard quietly kept showing the wrong thing.
    console.error(e);
    useMetricsStore
      .getState()
      .setHistoryError(e instanceof Error ? e.message : String(e));
  }
  // Every trigger that re-snaps the chart window (mount, timescale change,
  // reconnect, tab-return) should re-snap the network totals with it.
  loadNetworkTotals(timescale);
}

/**
 * Refresh just the network total tally, independent of `snapshots`.
 *
 * Deliberately does not touch the history buffer — `loadHistory` replaces
 * the whole chart window in one shot, which is fine on reconnect but would
 * cause a visible "jump" if run on a plain timer. The totals number has no
 * such animation to protect, so it gets its own periodic refresh (see
 * `NETWORK_TOTALS_POLL_MS` below) to stay accurate as the rolling window
 * slides forward.
 */
export async function loadNetworkTotals(timescale: Timescale): Promise<void> {
  try {
    const totals = await fetchNetworkTotals(timescale);
    useMetricsStore.getState().setNetworkTotals(totals);
  } catch (e) {
    console.error(e);
  }
}

/** How often to re-check the totals between the events that already trigger
 *  a refresh (mount, timescale change, reconnect). The rolling window's
 *  edge otherwise only moves when one of those fires. */
const NETWORK_TOTALS_POLL_MS = 15_000;

export function useInitialData() {
  const timescale = useTimescaleStore((s) => s.timescale);

  useEffect(() => {
    fetch('/api/system')
      .then((r) => r.json())
      .then((data: SystemInfo) => useSystemStore.getState().setInfo(data))
      .catch(console.error);
  }, []);

  // Reload the window whenever the selected timescale changes. (Re)connect-
  // and visibility-driven reloads are handled by the WebSocket hook so every
  // path that resumes live data also re-snaps the charts to the present.
  useEffect(() => {
    loadHistory(timescale);
  }, [timescale]);

  // Keep the network totals current between those events too — the window
  // is rolling ("last 1h"), so its edge moves every second even when
  // nothing else about the page changes.
  useEffect(() => {
    const id = setInterval(() => loadNetworkTotals(timescale), NETWORK_TOTALS_POLL_MS);
    return () => clearInterval(id);
  }, [timescale]);
}
