import { useRef, useLayoutEffect, useMemo, useCallback } from 'react';
import {
  AreaChart, Area, XAxis, YAxis, ResponsiveContainer, CartesianGrid, Tooltip,
  useYAxisScale, useXAxisScale, usePlotArea,
} from 'recharts';
import { useTimescaleStore } from '../store/timescale-store';
import { useHoverStore } from '../store/hover-store';
import { useChartSettingsStore } from '../store/chart-settings-store';
import {
  downsample, smooth, formatXTick, getXDomain, getXTickCount,
  aggregates, BAND_LO, BAND_HI,
} from '../lib/chart-utils';
import ChartHoverEcho, { type EchoSeries } from './ChartHoverEcho';

interface LineConfig {
  dataKey: string;
  color: string;
  dashed?: boolean;
  /** Plot this line against the (right-side) secondary Y axis. The chart
   *  renders the secondary axis automatically when at least one line
   *  opts in. Useful for overlaying a metric in different units —
   *  e.g. fan RPM on a temperature chart. */
  secondary?: boolean;
  /** Mark the window's lowest bucket value alongside the peak marker,
   *  mirroring it downward. Opt-in per line — for most metrics
   *  (utilisation, power, byte rates) the minimum sits at or near zero
   *  and says nothing; this is for series where a dip is real signal
   *  (temperature, fan RPM). Requires the line's data to carry a band
   *  (see `bandKeys` below). */
  trough?: boolean;
  /** Override the axis-based formatter for this line's tooltip row and
   *  peak/trough label. For a line plotted on a shared/normalized scale
   *  (e.g. fan RPM re-expressed as % of max so it can share CPU temp's
   *  0–100 axis instead of needing its own), the axis formatter no
   *  longer describes this line's real unit — this does. */
  formatter?: (v: number) => string;
}

export interface LegendItem {
  label: string;
  color: string;
  value?: string;
  /** dataKey in the chart data — used to match hover payload to legend labels. */
  dataKey?: string;
  /** Draw the swatch as a caret rather than a rule — for the peak/trough
   *  companion rows, where a line swatch would misdescribe the mark.
   *  'up' draws ⌃ (peak), 'down' draws ⌄ (trough). */
  shaded?: 'up' | 'down';
  /** Promote this metric to a large "hero" current-value readout overlaid on
   *  the chart, instead of a small legend pill. One primary → a single big
   *  number; two co-equal primaries (e.g. CPU/GPU, ▲/▼) → two side-by-side. */
  primary?: boolean;
}

interface MetricChartProps {
  data: Record<string, unknown>[];
  lines: LineConfig[];
  /** Fixed domain used when autoscale is OFF. Use [0, 'auto'] for metrics without a natural ceiling. */
  yDomain: [number, number | 'auto'];
  yFormatter?: (v: number) => string;
  /** Domain + formatter for the secondary (right-side) Y axis. Required
   *  when any line sets `secondary: true`. Ignored otherwise. */
  secondaryYDomain?: [number, number | 'auto'];
  secondaryYFormatter?: (v: number) => string;
  title: string;
  legend?: LegendItem[];
  /** Hide the X axis (e.g. when the timescale axis lives elsewhere in the layout) */
  hideXAxis?: boolean;
}

/** Wall-clock HH:MM:SS for the hovered point. */
function formatHoverClock(ts: number): string {
  const d = new Date(ts);
  const pad = (n: number) => n.toString().padStart(2, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** Relative offset from "now" (e.g. "-12s", "-3.4m", "-2.1h"). */
function formatHoverOffset(ts: number): string {
  const diff = Date.now() - ts;
  if (diff < 0) return '';
  if (diff < 60_000) return `-${Math.round(diff / 1000)}s`;
  if (diff < 3_600_000) return `-${(diff / 60_000).toFixed(1)}m`;
  return `-${(diff / 3_600_000).toFixed(1)}h`;
}

interface EndTag {
  /** Pre-formatted value string, e.g. "42%", "200.1 GB", "0.018 MB/s". */
  value: string;
  color: string;
  /** Which Y axis this line plots against, for the value→pixel scale. */
  axis: 'left' | 'right';
  /** Last numeric value in chart units — used to find the pixel Y. */
  num: number;
}

const TAG_H = 17;
/** Minimum vertical gap between stacked tags before they're pushed apart. */
const TAG_GAP = 19;
/** Small clearance between the plot's true right edge and the tag pill,
 *  so the pill's ring doesn't sit flush on top of the axis border. Filling
 *  the chart is priority one here — the pill is allowed to sit over the
 *  curve's own ink; this is not a reserved margin, just breathing room. */
const TAG_EDGE_PAD = 2;
/** Floor and ceiling for the tag pill's box width — used only to size the
 *  right-aligned host box, not to reserve plot space for it. */
const MIN_TAG_BOX_W = 40;
const MAX_TAG_BOX_W = 80;

/**
 * Estimate a tag pill's rendered width from its formatted value, so its
 * host box roughly matches without needing an actual layout pass. Fit
 * from measured DOM widths (11px bold value + 8px unit, `gap-0.5` +
 * `px-1.5` padding): "20W"→37px, "45°C"→39px, "67.7 GB"→52px,
 * "0.004 MB/s"→68px, against their glyph count with whitespace collapsed
 * (the layout gap replaces any literal space, so a counted space would
 * double-charge it). The box right-aligns its content regardless, so an
 * under-estimate just grows further left rather than clipping.
 */
function estimateTagWidth(value: string): number {
  const glyphs = value.replace(/\s+/g, '').length;
  return Math.round(20 + glyphs * 5.5);
}

/**
 * Live value tags floating at the plot's right edge, overlapping whatever
 * curve ink is underneath — filling the chart takes priority over keeping
 * the tags clear of it. The opaque scrim keeps each one readable regardless
 * of what's behind it. Overlapping tags (e.g. network ▲/▼ both near zero)
 * are still pushed apart vertically so they don't sit on top of each other,
 * with a thin leader line back to the real point whenever that push moved
 * one off its true value.
 */
function EndValueTags({
  tags, lastTimestamp, boxW,
}: {
  tags: EndTag[];
  lastTimestamp: number | undefined;
  boxW: number;
}) {
  const yLeft = useYAxisScale('left');
  const yRight = useYAxisScale('right');
  const xScale = useXAxisScale();
  const plot = usePlotArea();
  if (!plot) return null;

  const rightEdge = plot.x + plot.width;
  const minY = plot.y + TAG_H / 2;
  const maxY = plot.y + plot.height - TAG_H / 2;
  const tagX = rightEdge - TAG_EDGE_PAD - boxW;
  const ox = typeof lastTimestamp === 'number' ? xScale?.(lastTimestamp) : undefined;
  const originX = typeof ox === 'number' && Number.isFinite(ox) ? ox : rightEdge;

  const placed = tags
    .map((t) => {
      const scale = t.axis === 'right' ? yRight : yLeft;
      const py = scale?.(t.num);
      const oy = typeof py === 'number' && Number.isFinite(py) ? py : NaN;
      // `y` is the tag's stacking position (mutated by de-collision below);
      // `oy` stays put as the leader line's true origin.
      return { ...t, y: oy, oy };
    })
    .filter((t) => Number.isFinite(t.y));
  if (placed.length === 0) return null;

  // De-collide: sort top→bottom, enforce a minimum gap, then clamp the whole
  // stack back inside the plot if pushing apart overflowed the bottom/top.
  placed.sort((a, b) => a.y - b.y);
  for (let i = 1; i < placed.length; i++) {
    if (placed[i].y < placed[i - 1].y + TAG_GAP) {
      placed[i].y = placed[i - 1].y + TAG_GAP;
    }
  }
  const over = placed[placed.length - 1].y - maxY;
  if (over > 0) placed.forEach((p) => { p.y -= over; });
  if (placed[0].y < minY) {
    const d = minY - placed[0].y;
    placed.forEach((p) => { p.y += d; });
  }

  return (
    <g data-overlay="end-tags">
      {placed.map((p, i) => {
        // Split "0.018 MB/s" → big number + small unit so the tag stays tight.
        const m = p.value.match(/^([\d.,]+)\s*(.*)$/);
        const num = m ? m[1] : p.value;
        const unit = m ? m[2] : '';
        return (
          <g key={i}>
            {/* Only draw the connector once de-collision actually moved the
                tag off its real value — a zero-length stub is just noise. */}
            {Math.abs(p.y - p.oy) > 1 && (
              <line
                x1={originX}
                y1={p.oy}
                x2={rightEdge - TAG_EDGE_PAD}
                y2={p.y}
                stroke={p.color}
                strokeWidth={1}
                strokeOpacity={0.5}
              />
            )}
            <foreignObject
              x={tagX}
              y={p.y - TAG_H / 2}
              width={boxW}
              height={TAG_H}
              style={{ overflow: 'visible' }}
            >
              {/* Right-align against the plot's true edge so the scrim
                  always wraps the full value even if `boxW` under-estimated
                  it — it just grows further left over the curve. */}
              <div className="flex items-center justify-end h-full">
                <div
                  className="inline-flex items-center gap-0.5 px-1.5 py-px rounded-md leading-none bg-bg-card"
                  style={{
                    boxShadow: `0 0 0 1px ${p.color}`,
                    // A blurred *drop* shadow, not a backdrop blur — the
                    // pill sits inside an SVG foreignObject, and
                    // `backdrop-filter` doesn't reliably sample the SVG
                    // curve painted behind it (blur never showed up; the
                    // curve just showed straight through). drop-shadow
                    // traces the pill's own rounded silhouette, so the
                    // blur stays sized to the pill instead of spilling
                    // into a separate halo.
                    filter: 'drop-shadow(0 1px 3px rgba(0,0,0,0.6))',
                  }}
                >
                  <span className="font-bold tabular-nums text-[11px]" style={{ color: p.color }}>
                    {num}
                  </span>
                  {unit && (
                    <span className="text-[8px] font-semibold opacity-75" style={{ color: p.color }}>
                      {unit}
                    </span>
                  )}
                </div>
              </div>
            </foreignObject>
          </g>
        );
      })}
    </g>
  );
}

interface ExtremePoint {
  dataKey: string;
  color: string;
  secondary?: boolean;
  timestamp: number;
  value: number;
  /** Pre-formatted, e.g. "68°", "218G". */
  label: string;
  direction: 'up' | 'down';
}

/** Find the row holding a series' window max/min, and where it happened. */
function findExtreme(
  rows: Record<string, unknown>[],
  key: string,
  pick: 'max' | 'min',
): { timestamp: number; value: number } | undefined {
  let best: number | undefined;
  let ts: number | undefined;
  for (const row of rows) {
    const v = row[key];
    const t = row.timestamp;
    if (typeof v !== 'number' || !Number.isFinite(v) || typeof t !== 'number') continue;
    if (best === undefined || (pick === 'max' ? v > best : v < best)) {
      best = v;
      ts = t;
    }
  }
  return best !== undefined && ts !== undefined ? { timestamp: ts, value: best } : undefined;
}

const MARKER_R = 2.5;
const MARKER_LABEL_W = 70;
const MARKER_LABEL_H = 15;
/** Gap between the dot and the label box's near edge. */
const MARKER_GAP = 5;

/**
 * A single dot at the exact point a line hit its window max (or min, for
 * a trough) — plus a small labeled pill next to it. This replaced
 * continuous headroom/footroom shading (a translucent fill from the trend
 * line to the bucket extreme, drawn behind the whole line): the fill was
 * too subtle to notice at a glance, and even once seen, "peak 68°"
 * sitting in the legend didn't say *where* — a reader had to hunt the
 * shaded ribbon for the tallest point themselves. A dot planted at that
 * exact spot with its value spelled out next to it needs no hunting and
 * no decoding.
 *
 * Only the single window extreme per line, not one per bucket — CLAUDE.md
 * documents three earlier per-bucket designs (band, connected polyline,
 * unconnected ticks) that all read as restless noise once a busy machine
 * touched its ceiling in nearly every bucket. One dot per line avoids that
 * failure mode structurally: it can never multiply with the timescale.
 */
function ExtremeMarkers({ points }: { points: ExtremePoint[] }) {
  const yLeft = useYAxisScale('left');
  const yRight = useYAxisScale('right');
  const xScale = useXAxisScale();
  const plot = usePlotArea();
  if (!plot) return null;

  const placed = points
    .map((p) => {
      const yScale = p.secondary ? yRight : yLeft;
      const x = xScale?.(p.timestamp);
      const y = yScale?.(p.value);
      return { ...p, x, y };
    })
    .filter(
      (p): p is ExtremePoint & { x: number; y: number } =>
        typeof p.x === 'number' && Number.isFinite(p.x)
          && typeof p.y === 'number' && Number.isFinite(p.y),
    );
  if (placed.length === 0) return null;

  // Compute every label's box up front (not during render) so each one can
  // check earlier boxes for overlap — two lines peaking together (e.g. CPU
  // and GPU both hitting 100% in the same burst) would otherwise land two
  // identical pills exactly on top of each other.
  const boxes = placed.map((p) => {
    // Clamp horizontally so the label never runs past the plot edges.
    const boxX = Math.min(
      Math.max(p.x - MARKER_LABEL_W / 2, plot.x),
      plot.x + plot.width - MARKER_LABEL_W,
    );
    // Prefer the side `direction` points to, but flip when that side has
    // no room — a peak sitting right at the axis ceiling would otherwise
    // push its label above the plot entirely, under the title bar overlay
    // where it's invisible.
    const roomAbove = p.y - plot.y;
    const roomBelow = plot.y + plot.height - p.y;
    const needed = MARKER_GAP + MARKER_LABEL_H;
    const placeAbove = p.direction === 'up'
      ? roomAbove >= needed || roomBelow < needed
      : roomAbove >= needed && roomBelow < needed;
    let boxY = placeAbove
      ? Math.max(p.y - MARKER_GAP - MARKER_LABEL_H, plot.y)
      : Math.min(p.y + MARKER_GAP, plot.y + plot.height - MARKER_LABEL_H);
    return { boxX, boxY, placeAbove };
  });
  // Resolve collisions in DOT order, not array order: walk markers
  // top-to-bottom by where their dot actually sits, and if a lower-dot
  // marker's box would horizontally overlap the one above it, push it
  // below — never above. Pushing by array index instead let one marker's
  // ceiling-clamp flip (a peak right at the axis top has no room above,
  // so it flips below its own dot) land its label above a neighbor whose
  // dot was actually higher, pairing each label with the other's dot.
  const order = placed.map((_, i) => i).sort((i, j) => placed[i].y - placed[j].y);
  for (let k = 1; k < order.length; k++) {
    const cur = boxes[order[k]];
    const prev = boxes[order[k - 1]];
    const xOverlap = cur.boxX < prev.boxX + MARKER_LABEL_W && cur.boxX + MARKER_LABEL_W > prev.boxX;
    if (xOverlap) {
      cur.boxY = Math.max(cur.boxY, prev.boxY + MARKER_LABEL_H + 2);
    }
  }

  return (
    <g data-overlay="extreme-markers">
      {placed.map((p, i) => {
        const { boxX, boxY } = boxes[i];
        return (
          <g key={i}>
            <circle
              cx={p.x}
              cy={p.y}
              r={MARKER_R}
              fill={p.color}
              stroke="var(--color-bg-card)"
              strokeWidth={1}
            />
            <foreignObject
              x={boxX}
              y={boxY}
              width={MARKER_LABEL_W}
              height={MARKER_LABEL_H}
              style={{ overflow: 'visible' }}
            >
              <div className="flex items-center justify-center h-full">
                <div
                  className="inline-flex items-center gap-0.5 px-1.5 py-px rounded-md leading-none bg-bg-card"
                  style={{
                    boxShadow: `0 0 0 1px ${p.color}`,
                    filter: 'drop-shadow(0 1px 3px rgba(0,0,0,0.6))',
                  }}
                >
                  <span className="text-[8px]" style={{ color: p.color }}>
                    {p.direction === 'up' ? '⌃' : '⌄'}
                  </span>
                  <span
                    className="font-semibold tabular-nums text-[9px]"
                    style={{ color: p.color }}
                  >
                    {p.label}
                  </span>
                </div>
              </div>
            </foreignObject>
          </g>
        );
      })}
    </g>
  );
}

/** Radius of the terminal dot marking where each line ends. */
const TAIL_DOT_R = 2.5;

interface TailPoint {
  dataKey: string;
  color: string;
  secondary?: boolean;
  /** Last plotted value, in chart units. */
  value: number;
}

/**
 * A dot at the end of every line — the point the legend and summary card
 * report, made explicit so the parity is obvious.
 *
 * Drawn as an overlay rather than through Recharts' `dot` prop, and that is a
 * performance fix, not a style one. `dot` invokes its renderer once per *data
 * point*: to show twelve visible dots the grid was creating 5 424 invisible
 * `r=0` circles — 88 % of its entire SVG — and React reconciled all of them on
 * every render. Twelve elements do the same job.
 */
function TailDots({ points, timestamp }: { points: TailPoint[]; timestamp: number | undefined }) {
  const yLeft = useYAxisScale('left');
  const yRight = useYAxisScale('right');
  const xScale = useXAxisScale();
  const plot = usePlotArea();
  if (!plot || typeof timestamp !== 'number') return null;

  const x = xScale?.(timestamp);
  if (typeof x !== 'number' || !Number.isFinite(x)) return null;

  return (
    <g data-overlay="tail-dots">
      {points.map((p) => {
        const y = (p.secondary ? yRight : yLeft)?.(p.value);
        if (typeof y !== 'number' || !Number.isFinite(y)) return null;
        return (
          <circle
            key={p.dataKey}
            cx={x}
            cy={y}
            r={TAIL_DOT_R}
            fill={p.color}
            stroke="var(--color-bg-card)"
            strokeWidth={1}
          />
        );
      })}
    </g>
  );
}

/* Hoisted so Recharts sees the same object every render — a fresh object
 * literal in JSX is a changed prop, which is what its performance guide means
 * by `jsx-no-new-object-as-prop`. */
const CURSOR_STYLE = {
  stroke: 'var(--color-chart-axis)',
  strokeWidth: 1,
  strokeDasharray: '3 3',
  strokeOpacity: 0.6,
} as const;
const ACTIVE_DOT = { r: 3 } as const;
const TOOLTIP_WRAPPER_STYLE = { zIndex: 20, outline: 'none' } as const;
const ESCAPE_VIEW_BOX = { x: false, y: false } as const;

export default function MetricChart({
  data, lines, yDomain, yFormatter,
  secondaryYDomain, secondaryYFormatter,
  title, legend, hideXAxis,
}: MetricChartProps) {
  const hasSecondary = lines.some((l) => l.secondary);
  const timescale = useTimescaleStore((s) => s.timescale);
  const autoscale = useChartSettingsStore((s) => s.autoscale);
  /** Ref on the inner chart wrapper so we can reach this card's own SVG. */
  const chartRef = useRef<HTMLDivElement>(null);

  /**
   * Make Recharts let go of *this* card's tooltip.
   *
   * Recharts owns its active-tooltip state and only releases it on a mouse-out,
   * which a finger never sends — so the hover store, which runs the touch
   * hold-and-fade for the whole grid, needs a way to close the loop on whichever
   * card raised the hover. React synthesises `onMouseLeave` from the bubbling
   * `mouseout` event, so dispatching a native one at the surface reaches
   * Recharts' internal `handleMouseLeave`.
   *
   * Registered with `setHover` rather than subscribed, for the reason spelled
   * out on `reportHover` below: reading the hover store in this component
   * re-renders the entire AreaChart.
   */
  const dismissOwnTooltip = useCallback(() => {
    const svg = chartRef.current?.querySelector('svg');
    svg?.dispatchEvent(new MouseEvent('mouseout', { bubbles: true, cancelable: true }));
  }, []);

  // The grid hovers as one: this chart reports its own hover to the shared
  // store, and `ChartHoverEcho` echoes any hover raised by a sibling. `title`
  // is the identity — every card has a distinct one, and it needs no plumbing
  // through the call site.
  //
  // Deliberately *not* subscribed here. Reading the hover in this component
  // made every pointer move re-render all six AreaCharts — scales, paths,
  // axes and all — which is what made the synced hover crawl. The echo
  // subscribes for itself, so a hover now re-renders six leaf overlays.
  const reportHover = useCallback((ts: number) => {
    useHoverStore.getState().setHover(ts, title, dismissOwnTooltip);
  }, [title, dismissOwnTooltip]);
  /** Recharts hands mouse and touch handlers the same state object. */
  const reportActivePoint = useCallback(
    (state: { activeLabel?: string | number; isTooltipActive?: boolean }) => {
      const ts = Number(state?.activeLabel);
      if (!state?.isTooltipActive || !Number.isFinite(ts)) return;
      reportHover(ts);
    },
    [reportHover],
  );
  const endHover = useCallback(() => {
    useHoverStore.getState().clearHover(title);
  }, [title]);
  const xDomain = getXDomain(timescale);
  const xTickCount = getXTickCount(timescale);
  // 1) Bucket the series down to MAX_POINTS[timescale]
  // 2) Smooth the bucket means so per-bucket variance doesn't leave visible
  //    kinks. Adjacent buckets can differ a lot during bursty workloads, and a
  //    5-sample window smooths across ~50 s of real time at 1 h, which matches
  //    the "trend line" people actually want at longer ranges.
  // 3) Where buckets genuinely aggregate, replace each mean with that bucket's
  //    maximum as a companion line named "peak" in the legend.
  const m4 = aggregates(timescale);
  // Memoized: bucketing and smoothing walk every retained sample (up to the
  // 3 600-entry buffer), and they only depend on the data and the window. Any
  // other reason to re-render — a theme flip, an autoscale toggle — must not
  // pay for them again.
  const smoothedMeans = useMemo(
    () => smooth(downsample(data as { timestamp: number }[], timescale), 5),
    [data, timescale],
  );

  /** Series the backend ships per-bucket bounds for. */
  const bandKeys = lines
    .map((l) => l.dataKey)
    .filter((k) => smoothedMeans[0] && `${k}${BAND_LO}` in (smoothedMeans[0] as object));

  const smoothed = smoothedMeans;

  /** A line's own formatter if it has one (see `LineConfig.formatter`),
   *  else the axis-based one for whichever side it plots against. */
  const formatterFor = (l: LineConfig) =>
    l.formatter ?? (l.secondary ? (secondaryYFormatter ?? yFormatter) : yFormatter);

  /** Series that get a peak marker — where this line hit its highest
   *  value across the visible window. */
  const peakLines = m4 ? lines.filter((l) => bandKeys.includes(l.dataKey)) : [];

  /**
   * Series that get a trough marker too — the mirror of `peakLines`,
   * opt-in per line (`trough: true`) since a dip only means something for
   * a handful of metrics (temperature, fan RPM). See the `trough` field
   * doc on `LineConfig` for why this isn't on by default everywhere peak
   * is.
   */
  const troughLines = m4 ? lines.filter((l) => bandKeys.includes(l.dataKey) && l.trough) : [];

  /** Where + what each peak/trough line's window extreme actually is —
   *  feeds both the on-chart dot markers and their legend rows. */
  const peakExtremes: ExtremePoint[] = peakLines
    .map((l): ExtremePoint | undefined => {
      const found = findExtreme(smoothed as Record<string, unknown>[], `${l.dataKey}${BAND_HI}`, 'max');
      if (!found) return undefined;
      const format = formatterFor(l);
      return {
        dataKey: l.dataKey,
        color: l.color,
        secondary: l.secondary,
        timestamp: found.timestamp,
        value: found.value,
        label: format ? format(found.value) : found.value.toFixed(1),
        direction: 'up',
      };
    })
    .filter((p): p is ExtremePoint => p !== undefined);

  const troughExtremes: ExtremePoint[] = troughLines
    .map((l): ExtremePoint | undefined => {
      const found = findExtreme(smoothed as Record<string, unknown>[], `${l.dataKey}${BAND_LO}`, 'min');
      if (!found) return undefined;
      const format = formatterFor(l);
      return {
        dataKey: l.dataKey,
        color: l.color,
        secondary: l.secondary,
        timestamp: found.timestamp,
        value: found.value,
        label: format ? format(found.value) : found.value.toFixed(1),
        direction: 'down',
      };
    })
    .filter((p): p is ExtremePoint => p !== undefined);

  /** Legend rows for the peaks/troughs, reusing the marker's own label
   *  so the pill and the on-chart dot always agree. */
  const peakLegend: LegendItem[] = peakExtremes.map((e) => {
    const base = legend?.find((x) => x.dataKey === e.dataKey);
    return {
      label: `${base?.label ?? e.dataKey} peak`,
      color: e.color,
      value: e.label,
      dataKey: `${e.dataKey}${BAND_HI}`,
      shaded: 'up',
    };
  });
  const troughLegend: LegendItem[] = troughExtremes.map((e) => {
    const base = legend?.find((x) => x.dataKey === e.dataKey);
    return {
      label: `${base?.label ?? e.dataKey} trough`,
      color: e.color,
      value: e.label,
      dataKey: `${e.dataKey}${BAND_LO}`,
      shaded: 'down',
    };
  });

  /** Legend plus the synthesised peak/trough rows — used for pills and the tooltip. */
  const allLegend: LegendItem[] = [...(legend ?? []), ...peakLegend, ...troughLegend];

  /** Rows for the faded echo drawn when a *sibling* chart owns the hover —
   *  the same series, labels and formatters the real tooltip would list, in
   *  the same order Recharts hands them over (band carriers, then lines). */
  const echoLabel = (key: string) => allLegend.find((l) => l.dataKey === key)?.label ?? key;
  const echoSeries: EchoSeries[] = [
    ...peakLines.map((l) => ({
      dataKey: `${l.dataKey}${BAND_HI}`,
      color: l.color,
      label: echoLabel(`${l.dataKey}${BAND_HI}`),
      secondary: l.secondary,
      format: formatterFor(l),
      dot: false,
    })),
    ...troughLines.map((l) => ({
      dataKey: `${l.dataKey}${BAND_LO}`,
      color: l.color,
      label: echoLabel(`${l.dataKey}${BAND_LO}`),
      secondary: l.secondary,
      format: formatterFor(l),
      dot: false,
    })),
    ...lines.map((l) => ({
      dataKey: l.dataKey,
      color: l.color,
      label: echoLabel(l.dataKey),
      secondary: l.secondary,
      format: formatterFor(l),
    })),
  ];

  // Primary metrics get a live value tag riding the right end of their line.
  // We position by the last *smoothed* value (so the tag sits on the curve)
  // but display the pre-formatted legend string (so it matches the summary).
  //
  // Positioned from the *mean* series, not the plotted one: with M4 the final
  // vertex is whichever extreme the last bucket ended on, so the tag would
  // jump to a spike and stop marking where the value actually is.
  const lastPoint = smoothedMeans[smoothedMeans.length - 1] as Record<string, number> | undefined;
  const endTags: EndTag[] = (legend ?? [])
    .filter((l) => l.primary && l.dataKey && l.value !== undefined && l.value !== '—')
    .map((l) => {
      const cfg = lines.find((ln) => ln.dataKey === l.dataKey);
      const num = lastPoint ? Number(lastPoint[l.dataKey as string]) : NaN;
      return {
        value: l.value as string,
        color: l.color,
        axis: (cfg?.secondary ? 'right' : 'left') as 'left' | 'right',
        num,
      };
    })
    .filter((t) => Number.isFinite(t.num));

  /** Where every line ends — one dot each, drawn by `TailDots`. */
  const tailPoints: TailPoint[] = lines
    .map((l) => ({
      dataKey: l.dataKey,
      color: l.color,
      secondary: l.secondary,
      value: lastPoint ? Number(lastPoint[l.dataKey]) : NaN,
    }))
    .filter((p) => Number.isFinite(p.value));

  /** Sized to this chart's own tags — see `estimateTagWidth`. Only sizes
   *  the pill's host box now, not a reserved plot margin. */
  const tagBoxW = endTags.length > 0
    ? Math.min(MAX_TAG_BOX_W, Math.max(MIN_TAG_BOX_W, ...endTags.map((t) => estimateTagWidth(t.value))))
    : MIN_TAG_BOX_W;


  /**
   * Recharts inserts unrecognized children (our `EndValueTags` /
   * `ExtremeMarkers`) into the SVG at their raw JSX position, but its own
   * Area/Line paths are collected into a `recharts-zIndex-layer_100` group
   * that Recharts always places after them — so no JSX ordering can make
   * our overlays paint over the curves; the DOM position is fixed by
   * Recharts internals, not by us. SVG paints in document order, so moving
   * our `[data-overlay]` groups to be the SVG's last children forces them
   * on top instead. Runs after every commit (data updates every ~2s) —
   * cheap enough (a couple of `appendChild` calls are no-ops once already
   * last).
   */
  useLayoutEffect(() => {
    const svg = chartRef.current?.querySelector('svg.recharts-surface');
    if (!svg) return;
    svg.querySelectorAll('[data-overlay]').forEach((el) => svg.appendChild(el));
  });

  const resolvedYDomain: [number, number | 'auto'] = autoscale
    ? [0, 'auto']
    : (yDomain[1] === 'auto' ? [0, 'auto'] : [yDomain[0], yDomain[1]]);

  return (
    <div
      className="chart-cell relative flex flex-col min-h-0 h-full w-full select-none"
      tabIndex={-1}
    >
      {/* Chart fills the entire cell */}
      <div ref={chartRef} className="absolute inset-0">
        <ResponsiveContainer width="100%" height="100%">
          <AreaChart
            data={smoothed}
            margin={{ top: 18, right: 6, left: 0, bottom: hideXAxis ? 0 : 2 }}
            onMouseMove={reportActivePoint}
            /*
              A finger has its own path through Recharts and it does not pass
              through `onMouseMove`. Recharts 3 dispatches a dedicated
              `touchEventAction` on touchmove — it resolves the touched element
              with `elementFromPoint`, because touch has no enter/leave pair to
              infer it from — and moves its own tooltip off that, silently.

              So a scrub, which is the obvious gesture on a chart, moved the
              tooltip on the card under the finger and told this store nothing:
              the other five cards drew no echo at all, and the grid stopped
              hovering as one on every touch device. A stationary tap happened
              to work only because browsers replay it as mouse events.

              The handler receives the identical state object either way, so
              one function serves both.
            */
            onTouchMove={reportActivePoint}
            onMouseLeave={endHover}
          >
            <CartesianGrid
              stroke="var(--color-chart-grid)"
              strokeDasharray="2 4"
              vertical={false}
            />
            <XAxis
              dataKey="timestamp"
              type="number"
              domain={xDomain}
              tickCount={xTickCount}
              tickFormatter={(ts) => formatXTick(ts, timescale)}
              tick={hideXAxis ? false : { fontSize: 9, fill: 'var(--color-chart-axis)' }}
              stroke="var(--color-chart-grid)"
              allowDataOverflow
              hide={hideXAxis}
              height={hideXAxis ? 0 : 14}
            />
            <YAxis
              yAxisId="left"
              domain={resolvedYDomain}
              tickFormatter={yFormatter}
              tick={{ fontSize: 9, fill: 'var(--color-chart-axis)' }}
              stroke="transparent"
              tickCount={4}
              width={34}
              allowDataOverflow
              axisLine={false}
              tickLine={false}
            />
            {hasSecondary && (
              // Right-side axis for overlay metrics in different units
              // (e.g. fan RPM on a temperature chart). Slimmer than the
              // primary axis since it's auxiliary.
              <YAxis
                yAxisId="right"
                orientation="right"
                domain={
                  autoscale
                    ? [0, 'auto']
                    : (secondaryYDomain ?? [0, 'auto'])
                }
                tickFormatter={secondaryYFormatter}
                tick={{ fontSize: 9, fill: 'var(--color-chart-axis)' }}
                stroke="transparent"
                tickCount={4}
                width={32}
                allowDataOverflow
                axisLine={false}
                tickLine={false}
              />
            )}
            <Tooltip
              cursor={CURSOR_STYLE}
              // Keep the panel inside the chart cell. Recharts places the
              // tooltip to the lower-right of the cursor by default, so hovering
              // near the right or bottom edge — easy to do in a 3×2 grid of
              // small cells — pushed it past the boundary and it got clipped by
              // the neighbouring cell. `allowEscapeViewBox: false` makes
              // Recharts flip it to the other side of the cursor instead.
              allowEscapeViewBox={ESCAPE_VIEW_BOX}
              offset={12}
              // Opaque background. The panel previously inherited `bg-bg-card`,
              // the same colour as the card it floats over, so wherever it
              // overlapped a filled area the chart showed through the corners
              // and the numbers lost contrast.
              wrapperStyle={TOOLTIP_WRAPPER_STYLE}
              content={({ active, payload, label: ts }) => {
                if (!active || !payload?.length) return null;

                const clock = typeof ts === 'number' ? formatHoverClock(ts) : '';
                const offset = typeof ts === 'number' ? formatHoverOffset(ts) : '';

                return (
                  <div
                    style={{
                      pointerEvents: 'none',
                      // Solid, slightly lifted surface rather than the card
                      // colour, plus a real shadow — the panel has to read as
                      // floating *above* the plot, not painted into it.
                      background: 'var(--color-bg-hover)',
                      boxShadow: '0 8px 24px rgb(0 0 0 / 0.45)',
                    }}
                    className="border border-border-strong rounded-lg px-2.5 py-2 shadow-lg flex flex-col gap-1"
                  >
                    {clock && (
                      <div className="flex items-baseline justify-between gap-3 leading-none mb-1 pb-1 border-b border-border-soft/60">
                        <span className="text-[11px] font-semibold text-text-primary tabular-nums">
                          {clock}
                        </span>
                        {offset && (
                          <span className="text-[9px] text-text-muted tabular-nums">
                            {offset}
                          </span>
                        )}
                      </div>
                    )}
                    {(payload as unknown as Array<{ dataKey: string; value: number; color: string }>)
                      // Range bands must not produce rows. Their value is a
                      // `[min, max]` array, and every formatter below assumes a
                      // number — `entry.value.toFixed(2)` on an array throws and
                      // takes the whole dashboard down with it. `tooltipType="none"`
                      // on the Area does not suppress the entry in Recharts 3, so
                      // the guard has to live here. Filtering on the value's type
                      // rather than the key name also covers any future series
                      // that turns out not to be a plain number.
                      .filter((entry) => typeof entry.value === 'number' && Number.isFinite(entry.value))
                      .map((entry) => {
                      const legendItem = allLegend.find((l) => l.dataKey === entry.dataKey);
                      const itemLabel = legendItem?.label ?? entry.dataKey;
                      // Pick the right formatter for this line — its own
                      // override if it has one (e.g. fan RPM re-expressed
                      // as % to share CPU temp's axis still wants to show
                      // real RPM here), else whichever axis it plots
                      // against, since a single shared formatter would lie
                      // about half the rows.
                      const lineCfg = lines.find(
                        (l) => l.dataKey === entry.dataKey
                          || `${l.dataKey}${BAND_HI}` === entry.dataKey
                          || `${l.dataKey}${BAND_LO}` === entry.dataKey,
                      );
                      const fmt = lineCfg ? formatterFor(lineCfg) : yFormatter;
                      const formatted = fmt
                        ? fmt(entry.value)
                        : entry.value.toFixed(2);
                      return (
                        <div key={entry.dataKey} className="flex items-center gap-2 text-[11px] leading-none">
                          <span className="text-[10px] shrink-0" style={{ color: entry.color }}>━</span>
                          <span className="text-text-secondary">{itemLabel}</span>
                          <span className="font-semibold tabular-nums text-text-primary ml-auto pl-3">
                            {formatted}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                );
              }}
            />
            <defs>
              {lines.map((line) => (
                <linearGradient key={line.dataKey} id={`grad-${line.dataKey}`} x1="0" y1="0" x2="0" y2="1">
                  <stop offset="5%" stopColor={line.color} stopOpacity={0.2} />
                  <stop offset="95%" stopColor={line.color} stopOpacity={0} />
                </linearGradient>
              ))}
            </defs>
            {/*
              Invisible — present only so the window's peak/trough (whatever
              bucket you're currently hovering) reaches the tooltip as a
              plain number. The visible marks for these live in
              `ExtremeMarkers` below, drawn once per line at the single
              point where the window max/min actually happened; this Area
              just makes the *per-bucket* max/min available on hover too.
            */}
            {peakLines.map((line) => (
              <Area
                key={`peakval-${line.dataKey}`}
                type="linear"
                yAxisId={line.secondary ? 'right' : 'left'}
                dataKey={`${line.dataKey}${BAND_HI}`}
                stroke="none"
                fill="none"
                dot={false}
                activeDot={false}
                isAnimationActive={false}
              />
            ))}
            {/* Invisible tooltip-value carrier for the trough — see the peak's twin above. */}
            {troughLines.map((line) => (
              <Area
                key={`troughval-${line.dataKey}`}
                type="linear"
                yAxisId={line.secondary ? 'right' : 'left'}
                dataKey={`${line.dataKey}${BAND_LO}`}
                stroke="none"
                fill="none"
                dot={false}
                activeDot={false}
                isAnimationActive={false}
              />
            ))}
            {lines.map((line, i) => {
              return (
                <Area
                  key={line.dataKey}
                  // `monotone` (d3's monotoneX) is a cubic Hermite spline that
                  // preserves local extrema, so it never overshoots — the right
                  // curve for a dense, already-smoothed signal.
                  //
                  // The extremes live on their own `_m4` series above, so this
                  // one stays the trend line at every range.
                  type="monotone"
                  yAxisId={line.secondary ? 'right' : 'left'}
                  dataKey={line.dataKey}
                  stroke={line.color}
                  strokeWidth={i === 0 ? 1.5 : 1}
                  strokeDasharray={line.dashed ? '4 2' : undefined}
                  // Only the primary line gets the gradient fill; the
                  // overlay/auxiliary lines stay as plain strokes so the
                  // chart doesn't look layered with translucent paint.
                  // Only the primary line gets a gradient fill, and not while
                  // the M4 trace is drawn: a wash under a trend line that the
                  // trace keeps crossing muddles both, and the point of putting
                  // extremes on a hairline is that they cost almost no ink.
                  fill={i === 0 && !line.secondary && !m4 ? `url(#grad-${line.dataKey})` : 'transparent'}
                  fillOpacity={1}
                  // No per-point dot renderer — the terminal dot is drawn once
                  // by `TailDots`. See its doc for what this prop was costing.
                  dot={false}
                  activeDot={ACTIVE_DOT}
                  isAnimationActive={false}
                  // Connect across missing values so a sensor that
                  // intermittently reports (e.g. M3+ idle-gated GPU
                  // temp before the first live reading) draws a single
                  // continuous line instead of a chain of stub segments.
                  // Backed by ChartGrid's forward-fill which holds the
                  // last live value through idle gaps.
                  connectNulls
                />
              );
            })}
            {(peakExtremes.length > 0 || troughExtremes.length > 0) && (
              <ExtremeMarkers points={[...peakExtremes, ...troughExtremes]} />
            )}
            {endTags.length > 0 && (
              <EndValueTags
                tags={endTags}
                lastTimestamp={lastPoint?.timestamp}
                boxW={tagBoxW}
              />
            )}
            <TailDots points={tailPoints} timestamp={lastPoint?.timestamp} />
            {/* Always mounted, and it subscribes to the hover itself — that is
                what keeps a pointer move from re-rendering this whole chart. */}
            <ChartHoverEcho
              chartId={title}
              rows={smoothed as Record<string, unknown>[]}
              series={echoSeries}
            />
          </AreaChart>
        </ResponsiveContainer>
      </div>

      {/*
        Title and legend on one line, left-aligned past the Y axis.

        The title used to be centred and the pills left-aligned, as two
        absolutely-positioned layers that knew nothing about each other — so
        they simply overlapped once there were enough pills, and adding the
        peak rows made every chart collide. Laying them out in a single row
        makes the collision structurally impossible, and title-then-legend is
        the order every dashboard uses.

        Primary metrics show here too, not just as the end-of-line tag —
        the tag can scroll out of a glance if the card isn't in view yet,
        and having the number sit right next to the title means it's the
        first thing read, same as every other pill.

        Two rules keep the row from silently eating its own contents. It used
        to be one `overflow-hidden` line of `shrink-0` items, which is a
        guarantee that anything past the card's width is destroyed with no
        mark left behind: Network's six pills measured 617 px against a 326 px
        card on a phone and a 585 px card at 1280, so the totals and the peak
        rows — the ones that have to be *named* to be readable at all — were
        the first to go on every width below 1512.

        So: the row wraps rather than clips, and below `sm` only the primary
        series stay. A phone card is ~130 px tall and cannot spend three
        wrapped lines on a legend; the peaks it drops are still on the chart
        as their own labelled pills, and the window totals are one tap away
        in the detail drawer.
      */}
      <div className="pointer-events-none absolute top-1 left-[38px] right-2 z-10 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[10px] leading-tight">
        <span className="shrink-0 font-semibold text-text-primary/90 tracking-wide px-1 rounded bg-bg-primary/75">
          {title}
        </span>
        {allLegend.map((l, idx) => (
          <span
            key={`sub-${l.label}-${idx}`}
            title={`${l.label}${l.value !== undefined ? ` ${l.value}` : ''}`}
            className={`shrink-0 px-1 rounded bg-bg-primary/70 items-center gap-1 text-left ${
              l.primary ? 'flex' : 'hidden sm:flex'
            }`}
          >
            <span style={{ color: l.color }}>
              {l.primary ? '●' : l.shaded === 'up' ? '⌃' : l.shaded === 'down' ? '⌄' : '━'}
            </span>
            <span className="text-text-secondary">{l.label}</span>
            {l.value !== undefined && (
              <span className="font-semibold text-text-primary tabular-nums">
                {l.value}
              </span>
            )}
          </span>
        ))}
      </div>
    </div>
  );
}
