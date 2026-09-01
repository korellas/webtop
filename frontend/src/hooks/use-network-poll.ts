import { useEffect } from 'react';
import { useNetworkStore } from '../store/network-store';
import { fetchNetworkInterfaces } from '../lib/api';

/**
 * Continuously fetch network interface info in the background. Runs from the
 * app root so the data is already in the store by the time the user opens
 * the Network drawer — no loading state, instant render.
 *
 * Cadence: 5 s. The backend 15 s cache means most of these are cheap, and
 * byte rates reflect the 5 s sampler window kept by `net_interfaces.rs`.
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
