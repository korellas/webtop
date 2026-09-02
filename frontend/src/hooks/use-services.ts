import { useEffect } from 'react';
import { fetchServices } from '../lib/api';
import { useServicesStore } from '../store/services-store';

/**
 * Poll the services endpoint while the panel is open.
 *
 * Unlike the network poll this does *not* run from the app root. Each pass
 * shells out to `launchctl` and `ps` and opens a TCP connection per service —
 * cheap, but not free, and pointless when nobody is looking. History is
 * recorded server-side at a fixed 5 s regardless, so nothing is lost by only
 * polling while visible.
 *
 * The cadence matches that server-side sampling period; polling faster would
 * just re-render the same numbers.
 */
const POLL_INTERVAL_MS = 5_000;

export function useServicesPoll(active: boolean) {
  const apply = useServicesStore((s) => s.apply);
  const setFetchError = useServicesStore((s) => s.setFetchError);

  // One probe at startup, whether or not the panel is open, purely to learn
  // whether this machine has a manifest at all — `NavBar` hides the tab when
  // it does not, and a tab that only reveals its own absence once you press it
  // is worse than no tab.
  //
  // It is not free, but it is cheap exactly where it runs often: with no
  // manifest the endpoint has nothing to shell out to and returns immediately.
  // The machines that pay for the probe are the ones whose answer is "yes,
  // and here is the list", which the panel wants anyway.
  useEffect(() => {
    const ctrl = new AbortController();
    let cancelled = false;
    fetchServices(ctrl.signal)
      .then((data) => { if (!cancelled) apply(data); })
      .catch(() => { /* Availability is unknown, so the tab stays. */ });
    return () => { cancelled = true; ctrl.abort(); };
  }, [apply]);

  useEffect(() => {
    if (!active) return;

    const ctrl = new AbortController();
    let cancelled = false;

    async function tick() {
      try {
        const data = await fetchServices(ctrl.signal);
        if (!cancelled) apply(data);
      } catch (e) {
        // An abort is this effect tearing down, not a failure worth showing.
        if (cancelled || ctrl.signal.aborted) return;
        setFetchError(e instanceof Error ? e.message : 'could not reach webtop');
      }
    }

    tick();
    const id = setInterval(tick, POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      ctrl.abort();
      clearInterval(id);
    };
  }, [active, apply, setFetchError]);
}
