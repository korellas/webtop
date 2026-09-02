import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { AnchorPos } from '../../store/drawer-store';

interface DropdownPanelProps {
  open: boolean;
  onClose: () => void;
  /** Viewport position of the clicked chip (for pointer alignment). */
  anchor: AnchorPos | null;
  children: React.ReactNode;
  labelId?: string;
}

/** Preferred panel width — capped to keep line-length readable. */
const PANEL_MAX_WIDTH = 520;
/** Minimum margin between the panel and the viewport edges. */
const VIEWPORT_MARGIN = 8;
/**
 * Height of the status bar the panel must clear, matching `StatusBar`'s `h-10`.
 *
 * The bar is the last child of a `h-dvh` column, so it owns the bottom 40 px
 * of the viewport at every width. Sizing the panel against the full `100dvh`
 * put its last card underneath the bar — invisible on a desktop, where the
 * panel rarely reaches that far, and a permanently half-sliced final row on a
 * phone, where it always does.
 */
const STATUS_BAR_HEIGHT = 40;

/**
 * Top-bar dropdown panel. Anchors under the clicked chip and slides down.
 * Width is capped at a readable size rather than spanning the full dashboard,
 * and the panel is centered under the anchor (then clamped into the viewport
 * if the anchor sits near the left or right edge).
 */
export default function DropdownPanel({
  open,
  onClose,
  anchor,
  children,
  labelId,
}: DropdownPanelProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  const [mounted, setMounted] = useState(open);
  const [visible, setVisible] = useState(false);
  const [viewportW, setViewportW] = useState(() =>
    typeof window !== 'undefined' ? window.innerWidth : 1024,
  );

  useEffect(() => {
    if (open) {
      setMounted(true);
      requestAnimationFrame(() => setVisible(true));
    } else {
      setVisible(false);
      const t = setTimeout(() => setMounted(false), 180);
      return () => clearTimeout(t);
    }
  }, [open]);

  // Keep the panel aligned if the user resizes the window while it's open.
  useEffect(() => {
    function onResize() {
      setViewportW(window.innerWidth);
    }
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  // ESC to close
  useEffect(() => {
    if (!open) return;
    function handleKey(e: KeyboardEvent) {
      if (e.key === 'Escape') {
        e.stopPropagation();
        onClose();
      }
    }
    window.addEventListener('keydown', handleKey);
    return () => window.removeEventListener('keydown', handleKey);
  }, [open, onClose]);

  if (!mounted) return null;

  // --- Panel geometry (in viewport coords) -----------------------------------
  const panelTop = anchor?.bottom ?? 56;
  const panelWidth = Math.min(PANEL_MAX_WIDTH, viewportW - VIEWPORT_MARGIN * 2);
  const anchorCenterX = anchor?.x ?? viewportW / 2;
  // Center under the anchor, then clamp so the panel stays inside the viewport.
  let panelLeft = anchorCenterX - panelWidth / 2;
  panelLeft = Math.max(
    VIEWPORT_MARGIN,
    Math.min(panelLeft, viewportW - panelWidth - VIEWPORT_MARGIN),
  );
  // Where the pointer notch sits inside the panel's own coordinate space.
  const pointerLocalX = Math.max(
    12,
    Math.min(anchorCenterX - panelLeft, panelWidth - 12),
  );

  // Portal target
  const portalTarget = (() => {
    let el = document.getElementById('drawer-root');
    if (!el) {
      el = document.createElement('div');
      el.id = 'drawer-root';
      document.body.appendChild(el);
    }
    return el;
  })();

  return createPortal(
    <>
      {/* Light backdrop — dims everything below the panel top and captures outside clicks. */}
      <div
        aria-hidden="true"
        onClick={onClose}
        className={`
          fixed inset-x-0 bottom-0 z-[80]
          bg-scrim/60
          transition-opacity duration-200
          motion-reduce:transition-none
          ${visible ? 'opacity-100' : 'opacity-0'}
        `}
        style={{ top: panelTop }}
      />

      {/* Panel — positioned near the anchor, capped to a readable width. */}
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="false"
        aria-labelledby={labelId}
        tabIndex={-1}
        style={{
          top: panelTop,
          left: `${panelLeft}px`,
          width: `${panelWidth}px`,
          // translate3d + will-change force GPU compositing — fixes Safari jank
          // where plain translateY would fall back to the software path.
          transform: visible ? 'translate3d(0,0,0)' : 'translate3d(0,-8px,0)',
          opacity: visible ? 1 : 0,
          willChange: 'transform, opacity',
          transition:
            'transform 180ms cubic-bezier(0.22, 1, 0.36, 1), opacity 180ms ease-out',
          // The bottom inset rides along with the bar it has to clear: the app
          // column is padded into the safe area, so the bar's floor is that
          // much higher than `100dvh` alone would suggest.
          maxHeight: `calc(100dvh - ${panelTop + 16 + STATUS_BAR_HEIGHT}px - env(safe-area-inset-bottom))`,
        }}
        className="
          fixed z-[90]
          bg-bg-elevated border border-border-strong
          rounded-panel shadow-2xl
          flex flex-col
          motion-reduce:transition-none
          outline-none
        "
      >
        {/* Pointer notch — points up at the originating chip. */}
        <PointerNotch localX={pointerLocalX} />

        {/* Content */}
        <div className="flex-1 min-h-0 overflow-y-auto thin-scroll px-4 pb-4">
          {children}
        </div>
      </div>
    </>,
    portalTarget,
  );
}

/** Upward-pointing triangle pinned to the panel's left edge at `localX` px. */
function PointerNotch({ localX }: { localX: number }) {
  return (
    <div
      aria-hidden="true"
      className="absolute -top-[7px] pointer-events-none"
      style={{
        left: `${localX}px`,
        transform: 'translateX(-50%)',
      }}
    >
      <svg width="14" height="8" viewBox="0 0 14 8" fill="none">
        <path
          d="M7 0L14 8H0L7 0Z"
          fill="var(--color-bg-elevated)"
          stroke="var(--color-border-strong)"
          strokeWidth="1"
        />
        {/* Hide the bottom edge of the triangle so the panel border looks continuous */}
        <rect x="1" y="7.5" width="12" height="1" fill="var(--color-bg-elevated)" />
      </svg>
    </div>
  );
}
