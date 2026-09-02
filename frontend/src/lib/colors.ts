/**
 * The data palette, as domain names rather than metric names.
 *
 * These reference the custom properties in `app.css`, so the palette tracks
 * the active theme instead of being pinned to the dark-mode hex values.
 *
 * Seven hues and one neutral, and that is the whole budget — see
 * `docs/design-guide.md` §1. A card gets *one* hue; the series inside it
 * separate by lightness and dash pattern. The names below are domains
 * (`compute`, `storage`, `power`) precisely so that adding a metric does not
 * read as an invitation to add a colour: a new power series is another
 * lightness of `power`, and there is nowhere in this object to put a
 * fuchsia.
 *
 * The previous palette had grown to seventeen entries — `powerCpu`,
 * `powerGpu` and `powerOther` were violet, fuchsia and rose, which is three
 * hues arguing on one plot, and `networkUp` was emerald, the same green the
 * GPU uses two cards away.
 */

const v = (name: string) => `var(--color-${name})`;

export const COLORS = {
  /** CPU, and the P/E core breakdown drawn as derived lines. */
  compute: v('compute'),
  computeLight: v('compute-light'),
  /** Its own hue, not a shade of compute: a separate subsystem, not a part. */
  gpu: v('gpu'),
  gpuLight: v('gpu-light'),
  memory: v('memory'),
  memoryLight: v('memory-light'),
  thermal: v('thermal'),
  thermalLight: v('thermal-light'),
  storage: v('storage'),
  storageLight: v('storage-light'),
  network: v('network'),
  networkLight: v('network-light'),
  power: v('power'),
  powerLight: v('power-light'),

  /**
   * The one neutral. The fan shares temperature's axis without being a
   * temperature, so it is drawn in slate and dashed — the shape and the
   * colour say the same thing, which is "read me as dependent".
   */
  fan: v('fan'),

  /** Status. Text and 1px borders only — never a line or a fill. */
  danger: v('danger'),
  warning: v('warning'),
  ok: v('ok'),

  /** Chart chrome — grid lines and axis labels. */
  chartGrid: v('chart-grid'),
  chartAxis: v('chart-axis'),
} as const;
