import { useEffect } from 'react';
import { create } from 'zustand';

/**
 * Which point in time the pointer is on, shared by every chart in the grid.
 *
 * One hover reads across all six cards — "the power spike at 14:03, what was
 * the GPU doing?" is the question the grid exists to answer, and answering it
 * by hovering each card in turn loses the position between moves. The chart
 * actually under the pointer keeps Recharts' own full-strength tooltip; the
 * others draw a faded echo at the same timestamp (see `ChartHoverEcho`), so
 * the reader can tell at a glance which one they are pointing at.
 *
 * A finger has no hover, only a tap, so touch gets a *hold*: the readout stays
 * at full strength for `HOLD_MS` after the finger lifts, then fades out over
 * `FADE_MS`. Touching anywhere during either phase restores it and restarts the
 * clock. A mouse leaving a chart still clears immediately — that gesture means
 * "stop", and a mouse can simply hover again.
 */
interface HoverState {
  /** Timestamp (ms) under the pointer, or `null` when nothing is hovered. */
  timestamp: number | null;
  /** Identity of the chart the pointer is actually over. */
  sourceId: string | null;
  /** The readout is on its way out. Drives opacity only — the hover is still
   *  live, and any touch brings it back. */
  fading: boolean;
  setHover: (timestamp: number, sourceId: string, dismissOwner: () => void) => void;
  /** Only the chart that owns the current hover may clear it — a leave
   *  event arriving after the pointer already entered a neighbouring cell
   *  would otherwise blank the fresh hover. */
  clearHover: (sourceId: string) => void;
  /** A touch landed somewhere. Cancel the dismissal and undo any fade. */
  keepAlive: () => void;
  /** A touch lifted. Start the hold, then the fade, then clear. */
  release: () => void;
}

/**
 * How long the readout holds at full strength after the finger lifts, and how
 * long it then takes to fade.
 *
 * The ceiling is not taste. The collector samples about every two seconds, so a
 * readout that outlives one sample period is a frozen number sitting on a line
 * that has already moved past it — not merely stale chrome but a wrong value,
 * presented with the same authority as a right one. Hold plus fade is sized to
 * land just inside that.
 */
const HOLD_MS = 1600;
const FADE_MS = 500;

/**
 * One timer set for the whole grid, not one per card.
 *
 * The hover is a single shared fact, so its dismissal has to be too. Each
 * `MetricChart` used to own a `dismissTimer`, which meant touching one card
 * and then another left the first card's timer running against a hover that
 * now belonged to the second — it would fire and blank a readout the reader
 * had just raised. Module scope rather than store state because nothing
 * renders from these.
 */
let holdTimer: ReturnType<typeof setTimeout> | null = null;
let fadeTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Every chart currently holding a Recharts tooltip, and how to make it let go.
 *
 * Recharts keeps its active-tooltip state internally and only releases it on a
 * mouse-out, which a finger never sends. Each chart registers its escape hatch
 * here when it raises a hover, so the store can close the loop without
 * subscribing anything to it — reading this store inside `MetricChart` is what
 * made the synced hover crawl in the first place.
 *
 * A map rather than a single slot, which is the whole bug it replaces. One slot
 * meant every new hover overwrote the last, so when the readout finally went
 * away only the most recent card was told. A finger wandering over four cards
 * inside one hold left three tooltips and three cursor lines painted on the
 * grid with nothing left that could ever remove them: touch has no leave event,
 * so they simply stayed until reload. Measured over a four-card gesture, three
 * of the four cards were stranded.
 */
const liveTooltips = new Map<string, () => void>();

/**
 * Let go of every registered tooltip except (optionally) one.
 *
 * Entries are collected and removed before any handler runs. Dismissing a card
 * dispatches a `mouseout` that comes straight back through `clearHover`, so the
 * map has to be consistent before the first callback fires.
 */
function dismissAllExcept(keep: string | null) {
  const doomed: (() => void)[] = [];
  for (const [id, dismiss] of liveTooltips) {
    if (id === keep) continue;
    doomed.push(dismiss);
    liveTooltips.delete(id);
  }
  doomed.forEach((d) => d());
}

/**
 * When a finger last touched anything, and how long after that a mouse event
 * is presumed to be the browser's echo of it rather than a real pointer.
 *
 * Touching somewhere new does not just deliver `touchstart` — the browser then
 * replays the gesture as a mouse sequence, and moving that phantom pointer off
 * the previously hovered card fires `mouseout` on it. Recharts turns that into
 * `onMouseLeave`, which reaches `clearHover` and blanks the readout instantly.
 * The effect is precisely backwards: tapping the status bar to *keep* the
 * numbers is what destroyed them.
 *
 * So a mouse-leave that arrives on the heels of a touch is not evidence that
 * anyone left; the hold-and-fade owns the dismissal in that case. A real mouse
 * leaving a card still clears at once, which is what that gesture means.
 */
let lastTouchAt = 0;
const TOUCH_MOUSE_GRACE_MS = 800;

function cancelTimers() {
  if (holdTimer !== null) {
    clearTimeout(holdTimer);
    holdTimer = null;
  }
  if (fadeTimer !== null) {
    clearTimeout(fadeTimer);
    fadeTimer = null;
  }
}

export const useHoverStore = create<HoverState>()((set, get) => ({
  timestamp: null,
  sourceId: null,
  fading: false,
  setHover: (timestamp, sourceId, dismiss) => {
    liveTooltips.set(sourceId, dismiss);
    const s = get();
    // Cancel *only* when reviving a fade, never unconditionally. A tap fires
    // `touchend` before the browser synthesises the mouse events that raise the
    // hover, so `release` has already scheduled the dismissal by the time this
    // runs — cancelling here would throw that away and leave the readout up
    // forever. When a fade really is in flight, though, its timer has to go or
    // it will clear the hover this call just revived.
    if (s.fading) cancelTimers();
    // Mousemove fires far more often than the pointer crosses a data point.
    // Setting unconditionally would re-render all six charts per event. A
    // fading readout is not "unchanged" though — that one has to be revived.
    if (s.timestamp === timestamp && s.sourceId === sourceId && !s.fading) return;
    set({ timestamp, sourceId, fading: false });
    // Exactly one card carries the full-strength tooltip; the rest echo. Said
    // after the state moves, so the `mouseout` this sends cannot come back and
    // clear the hover that just replaced them.
    dismissAllExcept(sourceId);
  },
  clearHover: (sourceId) => {
    if (get().sourceId !== sourceId) return;
    // Inside the touch grace this is the phantom pointer leaving, not a reader,
    // so the hold-and-fade keeps the dismissal.
    //
    // Ignore it outright rather than relinquishing ownership. Handing the card
    // back so it would draw an echo like the others reads better in the one
    // case where the touch lands off every chart — but it puts a card one
    // `setHover` away from carrying Recharts' tooltip *and* an echo at the same
    // time, which is two readouts stacked on one plot. A card that briefly
    // shows nothing is a smaller lie than a card that shows the same number
    // twice.
    if (Date.now() - lastTouchAt < TOUCH_MOUSE_GRACE_MS) return;
    cancelTimers();
    liveTooltips.delete(sourceId);
    set({ timestamp: null, sourceId: null, fading: false });
  },
  keepAlive: () => {
    lastTouchAt = Date.now();
    // Nothing on screen: a tap is not a reason to invent a hover out of a
    // timestamp the reader never pointed at.
    if (get().timestamp === null) return;
    cancelTimers();
    if (get().fading) set({ fading: false });
  },
  release: () => {
    lastTouchAt = Date.now();
    // No early return on an empty hover, deliberately. On a tap the order is
    // touchstart, touchend, *then* the synthesised mouse events — so at this
    // moment the hover this gesture is about does not exist yet. The guard
    // belongs on the far side of the hold, where the answer is known.
    cancelTimers();
    holdTimer = setTimeout(() => {
      holdTimer = null;
      if (get().timestamp === null) return;
      set({ fading: true });
      fadeTimer = setTimeout(() => {
        fadeTimer = null;
        // Everything, not just the owner. This is the only moment the grid is
        // guaranteed to be swept clean of tooltips a finger left behind.
        dismissAllExcept(null);
        set({ timestamp: null, sourceId: null, fading: false });
      }, FADE_MS);
    }, HOLD_MS);
  },
}));

/**
 * Publish the fade as an attribute on `<html>` and let CSS do the transition.
 *
 * Deliberately not a subscription any chart can see. `MetricChart` reading this
 * store re-rendered six entire AreaCharts — scales, paths, axes — per pointer
 * move, which is documented in CLAUDE.md as the reason `ChartHoverEcho`
 * subscribes for itself. A fade that re-rendered them once per phase change
 * would be a smaller version of the same mistake, and it would land in the
 * middle of a gesture. `zustand`'s bare `subscribe` re-renders nothing, so the
 * whole effect costs one attribute write per phase.
 *
 * The duration travels with it as a custom property, so `app.css` styles the
 * transition without a second copy of the number.
 */
export function useHoverFade() {
  useEffect(() => {
    const root = document.documentElement;
    root.style.setProperty('--hover-fade-ms', `${FADE_MS}ms`);
    root.dataset.hoverFading = 'false';
    return useHoverStore.subscribe((s) => {
      root.dataset.hoverFading = s.fading ? 'true' : 'false';
    });
  }, []);
}
