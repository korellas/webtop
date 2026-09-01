import type { View } from '../store/view-store';

/**
 * Navigation vocabulary shared by the desktop rail and the phone's bottom bar.
 *
 * Kept apart from either component because the two render the same
 * destinations in different chrome, and a label or a behaviour that drifts
 * between them turns one control into two things to learn.
 *
 * Plain `.ts`, no JSX: mixing component and non-component exports in one file
 * breaks React Fast Refresh. The glyphs live in `NavIcon.tsx` for that reason.
 */
export const NAV_LABELS: Record<View, string> = {
  system: 'System',
  services: 'Services',
  processes: 'Processes',
};

/**
 * Clicking the view you are already in closes it rather than doing nothing —
 * the icon that opened a panel is the obvious place to reach for to dismiss it.
 * `system` is the base layer, so it has nothing to toggle off.
 */
export function nextView(current: View, clicked: View): View {
  return current === clicked && clicked !== 'system' ? 'system' : clicked;
}
