import { useEffect, useState } from 'react';
import type { ServiceStatus } from '../../lib/types';

/**
 * A restart takes far longer than the click that asks for it, and nothing in
 * the probed state covers the gap.
 *
 * The sequence is: SIGTERM goes out, the old process winds down, launchd
 * notices and respawns, the new process starts reading weights, and only then
 * does the port open. Measured on `phoenix` — the *cheapest* service here —
 * that took 22 seconds; a 27 B model server takes minutes. For the first few
 * seconds the poller still reports the old PID and a `running` state, so
 * without this the UI answers "restart?" with "everything is fine", the user
 * clicks again, and now two SIGTERMs are in flight.
 *
 * So the click is remembered until the PID actually changes. That is the only
 * unambiguous evidence the restart happened — a state transition could just be
 * the poller catching a normal blip.
 */

/** Give up waiting after this long and let the probed state speak for itself. */
const GIVE_UP_MS = 90_000;

interface Pending {
  /** PID at the moment the restart was requested. */
  prevPid: number | null;
  at: number;
}

export interface RestartTracker {
  /** Seconds since the restart was requested, or null if none is pending. */
  elapsedFor: (name: string) => number | null;
  begin: (name: string, prevPid: number | null) => void;
}

export function useRestartTracking(services: ServiceStatus[]): RestartTracker {
  const [pending, setPending] = useState<Record<string, Pending>>({});
  // Ticks once a second so the elapsed counter moves. Reading the clock during
  // render would make the component impure, and without a timer the counter
  // would only advance on the 5 s poll.
  const [now, setNow] = useState(() => Date.now());

  /**
   * Whether a request is still outstanding, derived rather than stored.
   *
   * Retiring entries in an effect would mean a `setState` on every poll, and
   * a render cascade for a fact that is already computable from the props we
   * were handed. An entry is finished when the PID moved — or when enough time
   * passed that the signal clearly did not take, so a service that ignores
   * SIGTERM stops claiming to be restarting forever.
   */
  function isOutstanding(name: string, p: Pending, clock: number): boolean {
    const svc = services.find((s) => s.name === name);
    const restarted = svc !== undefined && svc.pid !== null && svc.pid !== p.prevPid;
    return !restarted && clock - p.at <= GIVE_UP_MS;
  }

  const active = Object.entries(pending).some(([n, p]) => isOutstanding(n, p, now));

  // `setNow` lives in the interval callback, not the effect body — the effect
  // only subscribes to the timer.
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setNow(Date.now()), 1_000);
    return () => clearInterval(id);
  }, [active]);

  // `begin` is rebuilt every render, so its closure always sees the `services`
  // that were current when the button it is attached to was last drawn — which
  // is what the click acted on.
  return {
    elapsedFor: (name) => {
      const p = pending[name];
      if (!p || !isOutstanding(name, p, now)) return null;
      return Math.max(0, Math.round((now - p.at) / 1000));
    },
    begin: (name, prevPid) => {
      const at = Date.now();
      setNow(at);
      setPending((prev) => {
        // Drop entries that have resolved, so the map cannot grow across a
        // long session. Done here rather than in an effect for the reason
        // above.
        const kept: Record<string, Pending> = {};
        for (const [n, p] of Object.entries(prev)) {
          const svc = services.find((s) => s.name === n);
          const restarted = svc !== undefined && svc.pid !== null && svc.pid !== p.prevPid;
          if (!restarted && at - p.at <= GIVE_UP_MS) kept[n] = p;
        }
        return { ...kept, [name]: { prevPid, at } };
      });
    },
  };
}
