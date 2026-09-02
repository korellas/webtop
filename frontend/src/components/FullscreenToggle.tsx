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
