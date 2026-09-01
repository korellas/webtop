/**
 * Background tint strength for a table cell carrying a proportion.
 *
 * Borrowed from Windows Task Manager: shading the cell itself, rather than
 * adding a bar or an icon beside the number, means magnitude is visible while
 * scanning at no cost in column width. That last part is why both tables use
 * it — the services panel spent 26 % of its width on a memory bar column, and
 * the process manager spent 160 px, in each case squeezing the one column
 * whose content is genuinely variable-length.
 *
 * `**0.6` lifts the low end so moderate load stays faintly visible instead of
 * everything under ~40 % reading as empty. The floor of 0.05 makes "small but
 * non-zero" distinguishable from zero.
 *
 * Takes a fraction in 0..1, never a percentage. The two tables previously had
 * their own copies of this with different units, which is the kind of
 * difference that produces a silently 100×-wrong tint.
 */
export function heat(fraction: number): number {
  const t = Math.min(1, Math.max(0, fraction));
  return t === 0 ? 0 : 0.05 + Math.pow(t, 0.6) * 0.3;
}

/** Ready-to-use `background` value tinting `color` by `fraction` (0..1). */
export function heatBackground(color: string, fraction: number): string {
  return `color-mix(in oklab, ${color} ${heat(fraction) * 100}%, transparent)`;
}
