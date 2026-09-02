import { useEffect } from 'react';
import { useNetworkStore } from '../store/network-store';
import { fetchNetworkInterfaces } from '../lib/api';

/**
 * Fetch network interface info only while the Network drawer is mounted.
 *
 * Cadence: 5 s. Closing the drawer unmounts the hook and cancels both the
 * request and timer, so interface detail has no idle polling cost.
 */
const POLL_INTERVAL_MS = 5_000;

export function useNetworkPoll() {
  const setInterfaces = useNetworkStore((s) => s.setInterfaces);
  const markLoaded = useNetworkStore((s) => s.markLoaded);

  useEffect(() => {
    const ctrl = new AbortController();
    let cancelled = false;

    async function tick() {
      try {
        const data = await fetchNetworkInterfaces(ctrl.signal);
        if (!cancelled) setInterfaces(data);
      } catch {
        // Transient network/abort error — just mark loaded so spinner stops
        // and wait for the next tick.
        if (!cancelled) markLoaded();
      }
    }

    tick();
    const id = setInterval(tick, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      ctrl.abort();
      clearInterval(id);
    };
  }, [setInterfaces, markLoaded]);
}
