import { useEffect, type ReactNode } from 'react';

interface Props {
  title: string;
  onClose: () => void;
  children: ReactNode;
  /** Right-aligned controls in the overlay's title row (search, filters…). */
  actions?: ReactNode;
  /**
   * `fill` claims the full allowance; `hug` grows to the content and stops.
   *
   * A process list has hundreds of rows and always wants the space. A service
   * list has exactly as many rows as the manifest declares — pinning it to the
   * same height leaves a third of the panel empty below the last row, which
   * reads as something failing to load rather than as a list that ended.
   */
  height?: 'fill' | 'hug';
}

/**
 * One width for every overlay.
 *
 * Two different widths made the panels feel like two different components
 * rather than two views of one dashboard, and there was no principle behind
 * 720 vs 880 beyond whichever table I had last measured. Both hold a
 * six-column table; both get the same box.
 *
 * 800 px is what the process table needs with its 208 px detail panel open
 * (≈198 px left for the name column), and it leaves the services table ~274 px
 * for a name that needs 112 — comfortable without turning the remainder into
 * gutter the eye has to cross.
 */
const PANEL_WIDTH = 'min(calc(92dvw / var(--ui-zoom, 1)), 800px)';

/**
 * Large sliding panel over the dashboard.
 *
 * Same interaction language as the summary-chip drawers — click, something
 * covers the dashboard, dismiss to get back — but not the same geometry. Those
 * are dropdowns anchored to the chip that opened them, which is right for a
 * short list of stats and wrong for a scrollable table: an anchored panel is
 * bounded by the distance from the chip to the viewport edge.
 *
 * Overlaying rather than replacing the screen also means the charts underneath
 * keep running and stay visible at the edges, so opening the process list does
 * not feel like leaving the dashboard.
 */
export default function Overlay({
  title,
  onClose,
  children,
  actions,
  height = 'fill',
}: Props) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div className="absolute inset-0 z-40 flex items-center justify-center p-4">
      <button
        type="button"
        aria-label="Close"
        onClick={onClose}
        className="absolute inset-0 bg-scrim cursor-default"
      />

      {/*
        Sized, not full-bleed. A panel that fills the viewport is indistinguishable
        from a page navigation — the dashboard vanishes and the thing that
        replaced it has no edges, so there is nothing to tell you it is temporary
        or where clicking would take you back. Leaving a clear margin of dimmed
        dashboard on every side is what makes it read as "on top of" rather than
        "instead of", and it is what makes clicking outside an obvious way out.

        The clamps are the useful range: wide enough for six columns plus the
        detail panel, tall enough for ~20 rows, and capped so it never grows into
        the full-screen shape it is avoiding.
      */}
      <div
        role="dialog"
        aria-label={title}
        style={{
          width: PANEL_WIDTH,
          ...(height === 'fill'
            ? { height: 'min(calc(74dvh / var(--ui-zoom, 1)), 620px)' }
            : { maxHeight: 'min(calc(74dvh / var(--ui-zoom, 1)), 620px)' }),
        }}
        className="
          relative min-w-0 flex flex-col overlay-panel
          bg-bg-elevated border border-border-strong rounded-panel
          shadow-2xl overflow-hidden
        "
      >
        {/*
          `px-4`, matching the 16 px inset every child region uses, so the
          title sits on the same vertical line as the first column's text
          rather than 4 px to its left. 40 px tall is Carbon's small-toolbar
          height, paired there with exactly this kind of compact table.
        */}
        <div className="shrink-0 flex items-center gap-3 px-4 h-10 border-b border-border bg-bg-sidebar">
          <span className="text-[13px] font-semibold">{title}</span>
          {actions}
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            title="Close (Esc)"
            className="
              ml-auto w-7 h-7 rounded-control flex items-center justify-center
              text-text-muted hover:text-text-primary hover:bg-bg-hover
              transition-colors
            "
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none"
                 stroke="currentColor" strokeWidth="2.2" strokeLinecap="round">
              <path d="M6 6l12 12M18 6L6 18" />
            </svg>
          </button>
        </div>

        <div className="flex-1 min-h-0 flex">{children}</div>
      </div>
    </div>
  );
}
