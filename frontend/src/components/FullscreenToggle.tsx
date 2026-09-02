import { useCallback, useEffect, useState } from 'react';

/**
 * Vendor-prefixed Fullscreen API members, which are not in the DOM lib types.
 *
 * Safari only dropped the `webkit` prefix in 16.4, and this dashboard is aimed
 * at a Mac, so the prefixed path is a live one rather than legacy courtesy.
 */
interface PrefixedDocument extends Document {
  webkitFullscreenEnabled?: boolean;
  webkitFullscreenElement?: Element | null;
  webkitExitFullscreen?: () => Promise<void> | void;
}
interface PrefixedElement extends HTMLElement {
  webkitRequestFullscreen?: () => Promise<void> | void;
}

function doc(): PrefixedDocument {
  return document as PrefixedDocument;
}

/** Whether the browser will grant fullscreen at all. */
function isSupported(): boolean {
  const d = doc();
  return Boolean(d.fullscreenEnabled ?? d.webkitFullscreenEnabled);
}

function currentElement(): Element | null {
  const d = doc();
  return d.fullscreenElement ?? d.webkitFullscreenElement ?? null;
}

/**
 * The width this interface's pixel sizes are drawn for, and the ceiling on
 * how far it will scale past it.
 *
 * Going fullscreen used to hand the layout more room and nothing else: at
 * 2560x1440 a chart cell grew from 855x289 to 1271x435 while every number,
 * the chip strip and the status bar stayed the size they were. That is not
 * what fullscreen is for. Fullscreen is for reading the thing from further
 * away, and a dashboard that answers it with more empty plot has answered the
 * wrong question.
 *
 * The scale comes from `screen.width`, not `innerWidth`, because `innerWidth`
 * is reported in CSS pixels and therefore shrinks as the zoom it is being used
 * to compute grows — reading it here would be a feedback loop.
 */
const DESIGN_WIDTH = 1440;
const MAX_ZOOM = 2;

/**
 * Scale the whole interface up while fullscreen, and only while fullscreen.
 *
 * `zoom` rather than `transform: scale()`: zoom participates in layout, so the
 * charts re-measure and redraw at their new size instead of being a bitmap
 * stretched over more pixels. Every px in the design system scales with it,
 * which is the point — the type scale, the 40px bar and the 30px gauges keep
 * their relationships to each other and simply get bigger.
 *
 * Removed on exit, so nothing about a windowed session is changed by having
 * once been fullscreen.
 */
function useFullscreenZoom(active: boolean) {
  useEffect(() => {
    const root = document.documentElement;
    if (!active) {
      root.style.removeProperty('zoom');
      root.style.removeProperty('--ui-zoom');
      return;
    }
    const apply = () => {
      const raw = window.screen.width / DESIGN_WIDTH;
      const clamped = Math.min(MAX_ZOOM, Math.max(1, raw));
      // Quantised, so a window nudge cannot reflow the whole grid over a
      // difference nobody can see.
      const z = Math.round(clamped * 20) / 20;
      root.style.zoom = String(z);
      // Viewport units do not know about zoom. `100dvh` under `zoom: 1.8`
      // still resolves to the full viewport height *in zoomed pixels*, which
      // renders 1.8x too tall — the first build of this scaled the dashboard
      // beautifully and pushed the status bar and a whole chart row off the
      // bottom of the screen. Anything sized against the viewport divides by
      // this.
      root.style.setProperty('--ui-zoom', String(z));
    };
    apply();
    window.addEventListener('resize', apply);
    return () => {
      window.removeEventListener('resize', apply);
      root.style.removeProperty('zoom');
      root.style.removeProperty('--ui-zoom');
    };
  }, [active]);
}

/**
 * Fullscreen state, read from the browser rather than remembered.
 *
 * The button is never the only way out — Esc, F11 and the system window
 * controls all exit — so tracking this in local state would leave the icon
 * claiming the opposite of what the window is doing the first time someone
 * presses Escape. `fullscreenchange` is the single source of truth.
 */
function useFullscreen() {
  const [active, setActive] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const sync = () => setActive(currentElement() !== null);
    sync();
    document.addEventListener('fullscreenchange', sync);
    document.addEventListener('webkitfullscreenchange', sync);
    return () => {
      document.removeEventListener('fullscreenchange', sync);
      document.removeEventListener('webkitfullscreenchange', sync);
    };
  }, []);

  const toggle = useCallback(async () => {
    setError(null);
    try {
      if (currentElement() !== null) {
        const d = doc();
        await (d.exitFullscreen ? d.exitFullscreen() : d.webkitExitFullscreen?.());
        return;
      }
      const root = document.documentElement as PrefixedElement;
      await (root.requestFullscreen
        ? root.requestFullscreen({ navigationUI: 'hide' })
        : root.webkitRequestFullscreen?.());
    } catch (e: unknown) {
      // Not swallowed: `requestFullscreen` rejects when the gesture isn't
      // trusted or a policy blocks it, and a button that silently does nothing
      // is the worst of the available outcomes. The reason lands in the
      // tooltip, which is where someone who just pressed it is looking.
      setError(e instanceof Error ? e.message : 'Fullscreen was refused');
    }
  }, []);

  return { active, error, toggle };
}

/**
 * Fullscreen toggle, sitting with the other view controls in the status bar.
 *
 * Rendered only where the API actually exists. iPhone Safari has no element
 * fullscreen at all — `fullscreenEnabled` is false there — and a control that
 * is present but inert teaches the reader that the bar's buttons are
 * unreliable. Nothing is lost by its absence: on that browser the only way to
 * reclaim the chrome is the scroll gesture the layout already supports through
 * `h-dvh`.
 */
export default function FullscreenToggle() {
  const { active, error, toggle } = useFullscreen();
  useFullscreenZoom(active);
  const [supported] = useState(isSupported);
  if (!supported) return null;

  const label = active ? 'Exit fullscreen' : 'Fullscreen';

  return (
    <button
      type="button"
      onClick={toggle}
      aria-pressed={active}
      aria-label={label}
      title={error ? `${label} — ${error}` : label}
      className={`
        flex items-center justify-center px-1 min-h-7 min-w-6
        bg-bg-card border rounded-control cursor-pointer transition-colors
        ${error
          ? 'border-danger text-danger'
          : active
            ? 'border-border-strong bg-bg-hover text-text-primary'
            : 'border-border text-text-secondary hover:text-text-primary'}
      `}
    >
      {active ? <CompressIcon /> : <ExpandIcon /> }
    </button>
  );
}

/* Two glyphs rather than one rotated: arrows pointing out of the corners and
   arrows pointing into them are the pair every player and viewer uses, so the
   state is legible without reading the tooltip. */

function ExpandIcon() {
  return (
    <svg
      width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
    >
      <path d="M8 3H5a2 2 0 0 0-2 2v3M16 3h3a2 2 0 0 1 2 2v3M8 21H5a2 2 0 0 1-2-2v-3M16 21h3a2 2 0 0 0 2-2v-3" />
    </svg>
  );
}

function CompressIcon() {
  return (
    <svg
      width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"
    >
      <path d="M3 8h3a2 2 0 0 0 2-2V3M21 8h-3a2 2 0 0 1-2-2V3M3 16h3a2 2 0 0 1 2 2v3M21 16h-3a2 2 0 0 0-2 2v3" />
    </svg>
  );
}
