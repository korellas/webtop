import { useState } from 'react';
import StatusBar from './components/StatusBar';
import Overlay from './components/Overlay';
import SystemView from './components/SystemView';
import ProcessView from './components/processes/ProcessView';
import ServicesView from './components/services/ServicesView';
import DrawerContent from './components/drawer/DrawerContent';
import { useWebSocket } from './hooks/use-websocket';
import { useInitialData } from './hooks/use-history';
import { useServicesPoll } from './hooks/use-services';
import { useTheme } from './store/theme-store';
import { useViewStore } from './store/view-store';
import { useHoverStore, useHoverFade } from './store/hover-store';

export default function App() {
  useWebSocket();
  useInitialData();
  useTheme(); // keep data-theme attribute in sync
  useHoverFade(); // keep data-hover-fading attribute in sync

  const view = useViewStore((s) => s.view);
  const setView = useViewStore((s) => s.setView);
  const [procQuery, setProcQuery] = useState('');

  // Probing services costs a `launchctl`, a `ps` and a TCP connect per service,
  // so it only runs while the panel is open. History is recorded server-side on
  // its own schedule, so nothing is missed by not polling here.
  useServicesPoll(view === 'services');

  const closeOverlay = () => setView('system');

  // `h-dvh`, not `h-screen` (=100vh). On mobile Chrome/Safari `100vh` is the
  // *largest* viewport — it counts the strip the URL bar sits over — so the
  // layout ends up taller than what is actually visible and the bottom is
  // clipped. Worse, the root is `overflow-hidden`, so nothing scrolls, and
  // mobile browsers only retract the URL bar *in response to a scroll*. The bar
  // therefore never collapses and the clipping is permanent. `100dvh` tracks
  // the visible viewport, so the layout always fits.
  return (
    <div
      /*
        `index.html` asks for `viewport-fit=cover`, which grants the page the
        area under the notch, the rounded corners and the home indicator — and
        obliges it to keep content out of them. Nothing did, so on a device
        with insets the top of the chart grid and the bottom of the status bar
        sat underneath hardware.

        The bottom inset is deliberately not here. Padding the column for it
        lifts the status bar and leaves a band of page background beneath —
        an empty strip that reads as a layout mistake, because it is one. The
        bar owns that inset instead: it extends its own surface into it and
        pads its content clear, which is what every platform bottom bar does.

        Every inset resolves to 0px where there is none, so this is inert on
        a desktop.
      */
      // Height divides by the zoom factor for the reason spelled out in
      // `FullscreenToggle`: `dvh` is not zoom-aware, so under zoom the
      // unscaled `100dvh` renders taller than the screen it is meant to fit.
      style={{ height: 'calc(100dvh / var(--ui-zoom, 1))' }}
      className="
        flex flex-col bg-bg-primary text-text-primary overflow-hidden
        pt-[env(safe-area-inset-top)]
        pl-[env(safe-area-inset-left)] pr-[env(safe-area-inset-right)]
      "
      {...HOVER_TOUCH_HANDLERS}
    >
      {/*
        SystemView stays mounted underneath every overlay. Keeping it in the
        tree means the WebSocket-fed charts never unmount and remount, so
        switching panels does not throw away the rendered history and replay
        it — and the dashboard stays visible at the overlay's edges, so
        opening the process list reads as looking at something rather than
        navigating away.
      */}
      <main className="flex-1 min-h-0 relative flex">
        <SystemView />

        {view === 'processes' && (
          <Overlay
            title="Processes"
            onClose={closeOverlay}
            actions={<ProcessSearch value={procQuery} onChange={setProcQuery} />}
          >
            <ProcessView query={procQuery} />
          </Overlay>
        )}

        {/* `hug`: the manifest declares eight services, so the panel has a
            known, small height. Filling the full allowance would leave a
            third of it empty below the last row. */}
        {view === 'services' && (
          <Overlay title="Services" onClose={closeOverlay} height="hug">
            <ServicesView />
          </Overlay>
        )}
      </main>

      <StatusBar />

      {/* Detail drawers (portalled; lives outside the main layout) */}
      <DrawerContent />
    </div>
  );
}

/**
 * The hold-and-fade clock for the touch readout, bound at the root rather than
 * per chart.
 *
 * "Any touch keeps it" has to mean *any* touch — including one that lands on
 * the status bar or on empty space beside a card, which is exactly where a
 * finger goes when it is trying to get out of the way of the numbers it is
 * reading. Binding this to the charts alone would make the reset work only
 * where the readout is already covered by the hand.
 *
 * Both are no-ops when nothing is hovered, so this costs a function call per
 * tap and does not interfere with any other control.
 */
const HOVER_TOUCH_HANDLERS = {
  onTouchStart: () => useHoverStore.getState().keepAlive(),
  onTouchEnd: () => useHoverStore.getState().release(),
} as const;

/** Lives here rather than inside ProcessView so it can sit in the overlay's title row. */
function ProcessSearch({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  return (
    <div className="relative ml-3">
      <input
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="Filter by name, PID or user"
        className="
          w-56 pl-6 pr-2 py-0.5 rounded-md text-[11px]
          bg-bg-card border border-border text-text-primary
          placeholder:text-text-muted
          focus:outline-none focus:border-border-strong
        "
      />
      <svg
        className="absolute left-2 top-1/2 -translate-y-1/2 text-text-muted"
        width="11" height="11" viewBox="0 0 24 24" fill="none"
        stroke="currentColor" strokeWidth="2.2" strokeLinecap="round"
      >
        <circle cx="11" cy="11" r="7" />
        <path d="M20 20l-3.5-3.5" />
      </svg>
    </div>
  );
}
