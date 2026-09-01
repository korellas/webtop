import { useEffect, useRef } from 'react';
import { useMetricsStore } from '../store/metrics-store';
import { useTimescaleStore } from '../store/timescale-store';
import { loadHistory } from './use-history';
import type { SystemSnapshot } from '../lib/types';

/**
 * A healthy connection gets a push roughly every ~2s (the collector's real
 * cadence — see CLAUDE.md). 3x that margin absorbs normal jitter without
 * delaying real-staleness detection by much.
 */
const STALE_MS = 6_000;
/** How often the watchdog checks for staleness — a bit tighter than
 *  STALE_MS so detection latency stays close to STALE_MS itself. */
const WATCHDOG_INTERVAL_MS = 4_000;

export function useWebSocket() {
  const wsRef = useRef<WebSocket | null>(null);
  /** Set when we close on purpose (tab hidden / unmount) so the close handler
   *  doesn't fight us by scheduling a reconnect. */
  const intentionalCloseRef = useRef(false);
  /** Wall-clock time of the last message actually received (or the last
   *  forced reconnect), used by the watchdog below to tell "connected" from
   *  "looks connected but nothing is arriving". Set for real once the effect
   *  runs (0 would read as instantly stale, but the watchdog's first tick is
   *  WATCHDOG_INTERVAL_MS away, well after `connect()` below has set it). */
  const lastMessageAtRef = useRef(0);

  useEffect(() => {
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = `${protocol}//${window.location.host}/ws`;
    let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
    lastMessageAtRef.current = Date.now();

    function connect() {
      if (wsRef.current) return; // already connected/connecting
      useMetricsStore.getState().setWsStatus('connecting');
      const ws = new WebSocket(url);
      wsRef.current = ws;

      ws.onopen = () => {
        useMetricsStore.getState().setWsStatus('connected');
        lastMessageAtRef.current = Date.now();
        // Every (re)connect — first load, tab-return, sleep/wake, network
        // blip — snaps the charts to the current window before live frames
        // resume from the edge. This is what prevents the "replay" sweep.
        loadHistory(useTimescaleStore.getState().timescale);
      };

      ws.onmessage = (event) => {
        // Belt-and-suspenders: ignore anything that arrives while hidden so a
        // brief race between hide and close can't seed a backlog.
        if (document.visibilityState === 'hidden') return;
        lastMessageAtRef.current = Date.now();
        const snap: SystemSnapshot = JSON.parse(event.data);
        useMetricsStore.getState().pushSnapshot(snap);
      };

      ws.onclose = () => {
        // A close event from a socket we've already replaced (see
        // forceReconnect below) must not clobber the live one's ref or
        // schedule a redundant reconnect.
        if (wsRef.current !== ws) return;
        useMetricsStore.getState().setWsStatus('disconnected');
        wsRef.current = null;
        if (!intentionalCloseRef.current) {
          reconnectTimer = setTimeout(connect, 2000);
        }
      };

      ws.onerror = () => ws.close();
    }

    function disconnect() {
      intentionalCloseRef.current = true;
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      wsRef.current?.close();
      wsRef.current = null;
    }

    /**
     * Tear down whatever socket we currently hold — healthy or not — and
     * open a fresh one, then re-snap the chart window immediately.
     *
     * Never trusts `wsRef.current` as evidence of a live connection. Mobile
     * browsers can fully suspend a backgrounded tab without ever firing
     * 'hidden', so `disconnect()` never runs and `wsRef.current` keeps
     * pointing at a socket the OS silently dropped while we were away.
     * `connect()`'s "already connected" guard would then see that
     * stale-but-present ref and do nothing — the dashboard sits frozen
     * until the dead TCP connection eventually times out (minutes). Closing
     * and nulling it here guarantees `connect()` always opens a real socket.
     */
    function forceReconnect() {
      if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      wsRef.current?.close();
      wsRef.current = null;
      intentionalCloseRef.current = false;
      // Reset the clock now, not after the new socket opens — otherwise the
      // watchdog below can see the still-stale timestamp on its very next
      // tick and fire a second, redundant reconnect before the first one's
      // handshake has even finished.
      lastMessageAtRef.current = Date.now();
      // Independently of the socket, refresh the data right now. The
      // reconnect handshake can take a moment, and until it completes the
      // screen would otherwise sit frozen on the last frame from before the
      // tab went hidden — so snap the charts to the present immediately
      // instead of waiting.
      loadHistory(useTimescaleStore.getState().timescale);
      connect();
    }

    function onVisibilityChange() {
      if (document.visibilityState === 'visible') {
        forceReconnect();
      } else {
        disconnect();
      }
    }

    /**
     * Belt-and-suspenders for `onVisibilityChange`, not a replacement for
     * it: the original "reconnect on resume" fix depended entirely on
     * 'visibilitychange' firing, and it does not reliably fire on every
     * mobile browser for every kind of backgrounding (app-switcher, screen
     * lock, OS-level tab suspension) — so a fix that *only* reacts to that
     * event can look correct while doing nothing for the actual failure
     * case. This watchdog doesn't wait for any particular event: it checks
     * whether real data is actually still arriving and self-heals if not,
     * regardless of which (if any) lifecycle event fired.
     */
    function checkStaleness() {
      if (document.visibilityState !== 'visible') return;
      if (Date.now() - lastMessageAtRef.current > STALE_MS) {
        forceReconnect();
      }
    }

    connect();
    document.addEventListener('visibilitychange', onVisibilityChange);
    // 'pageshow' fires on back/forward-cache restores that some mobile
    // WebKit/Blink versions deliver without a matching 'visibilitychange' —
    // cheap extra trigger for the same forceReconnect path.
    window.addEventListener('pageshow', forceReconnect);
    const watchdog = setInterval(checkStaleness, WATCHDOG_INTERVAL_MS);

    return () => {
      document.removeEventListener('visibilitychange', onVisibilityChange);
      window.removeEventListener('pageshow', forceReconnect);
      clearInterval(watchdog);
      intentionalCloseRef.current = true;
      if (reconnectTimer) clearTimeout(reconnectTimer);
      wsRef.current?.close();
      wsRef.current = null;
    };
  }, []);
}
