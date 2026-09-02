import { useRef, useLayoutEffect, useMemo, useCallback } from 'react';
import {
  AreaChart, Area, XAxis, YAxis, ResponsiveContainer, CartesianGrid, Tooltip, ReferenceLine,
} from 'recharts';
import { useTimescaleStore } from '../store/timescale-store';
import { useHoverStore } from '../store/hover-store';
import { useChartSettingsStore } from '../store/chart-settings-store';
import {
  downsample, smooth, formatXTick, getXDomain, getXTickCount,
  aggregates, BAND_LO, BAND_HI,
} from '../lib/chart-utils';
import ChartHoverEcho, { type EchoSeries } from './ChartHoverEcho';

/**
 * How heavily a series is drawn, from `docs/series.json`.
 *
 * The contract is deliberately narrow, because the failure it prevents is a
 * chart where four series are all "important". Exactly one line per card may
 * hold area; two or more translucent fills stacked on one plot is mud. The
 * derived tier is capped at two per card for the same reason — the Power card
 * previously carried four 1px dashed lines in four different hues.
 */
export type SeriesTier = 'primary' | 'secondary' | 'derived';

/** stroke width, dash and opacity per tier — series.json's `tiers` table. */
const TIER_STYLE: Record<SeriesTier, { width: number; opacity: number }> = {
  primary: { width: 2, opacity: 1 },
  secondary: { width: 1.5, opacity: 1 },
  derived: { width: 1, opacity: 0.45 },
};

interface LineConfig {
  dataKey: string;
  color: string;
  /** Defaults to `derived`; a card names exactly one `primary`. */
  tier?: SeriesTier;
  /** Dash pattern for a derived line — `4 3`, `2 2`, `6 3`. Within one card
   *  these are what separate series that share a hue, so they are assigned
   *  per card in `ChartGrid`, not derived from the tier. */
  dash?: string;
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
  /**
   * Where this row sits in the card's readout hierarchy.
   *
   * 1 — the card's headline, 20px mono in the series colour. Exactly one.
   * 2 — the co-star (GPU, Swap, Write, Up), 13px mono.
   * 3 — derived values, peaks and troughs: 11px mono, muted, right-aligned
   *     behind a rule, because a peak is a different *kind* of fact from a
   *     current value and reading them as one list was the flat legend this
   *     replaces.
   *
   * Defaults to 3, so a row has to earn its size.
   */
  tier?: 1 | 2 | 3;
}

interface MetricChartProps {
  data: Record<string, unknown>[];
  lines: LineConfig[];
  /** Fixed domain used when autoscale is OFF. Use [0, 'auto'] for metrics without a natural ceiling. */
  yDomain: [number, number | 'auto'];
  yFormatter?: (v: number) => string;
  /**
   * Explicit gridline values.
   *
   * Recharts' automatic ticks put 35 / 70 / 100 on a percentage axis, which
   * are not landmarks anyone holds — half is. Given explicitly, a reader can
   * estimate a value off the gridlines instead of only off the label they
   * happen to be beside.
   */
  yTicks?: number[];
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

/*
 * `EndValueTags` and `ExtremeMarkers` used to live here — a pill welded to the
 * end of each line, and a labelled badge at each window peak and trough.
 *
 * Both are deleted rather than hidden. They printed numbers the legend already
 * carried, and they paid for the repetition on the plot itself: the end pills
 * stacked and de-collided against the x-axis labels in a 180px cell, and the
 * peak badge, by construction, sat on top of the curve whose peak it was
 * naming. Roughly 300 lines of collision-avoidance existed to make two
 * duplicated readouts not overlap a third thing.
 *
 * The values are all still on screen, in the legend, ranked — see `LegendItem.tier`.
 */


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


/*
 * `TailDots` used to live here: one filled dot pinned to the end of every
 * line, on the reasoning that a curve should say where it stops.
 *
 * It does say. The line ends. On a card carrying four series the dots were
 * four marks stacked in the right margin, and with the end-value pills gone
 * they were the last thing still competing with the plot for the same few
 * pixels. The current value is the 20px headline at the top of the card.
 */
/**
 * "0.058 MB/s" -> ["0.058", "MB/s"], "43\u00b0C" -> ["43", "\u00b0C"], "12%" -> ["12", "%"].
 *
 * The reading and its unit want different weights: one changes every two
 * seconds and the other never does. Splitting on the first character that is
 * not part of a number covers every formatter in `lib/format.ts` without any
 * of them having to know about this.
 */
function splitUnit(v: string): [string, string] {
  const m = v.match(/^([\d.,\-]+)\s*(.*)$/);
  return m ? [m[1], m[2]] : [v, ''];
}

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
  data, lines, yDomain, yFormatter, yTicks,
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

  /** Legend plus the synthesised peak/trough rows — used for the readout and
   *  the tooltip. The synthesised ones are tier 3 by construction: a window
   *  extreme is never the card's headline. */
  const allLegend: LegendItem[] = [...(legend ?? []), ...peakLegend, ...troughLegend];
  const rank = (l: LegendItem) => l.tier ?? 3;
  const tier1 = allLegend.filter((l) => rank(l) === 1);
  const tier2 = allLegend.filter((l) => rank(l) === 2);
  const tier3 = allLegend.filter((l) => rank(l) === 3);

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


  /**
   * Recharts inserts unrecognized children (our `ChartHoverEcho`) into the
   * SVG at their raw JSX position, but its own
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
            // 6px of floor even when the X axis is hidden. The zero tick is
            // centred on its own value, so with a flush bottom half the label
            // sits outside the plot and Recharts drops it silently — every
            // card was rendering 100/50 and no baseline at all.
            margin={{ top: 18, right: 6, left: 0, bottom: hideXAxis ? 6 : 2 }}
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
            {/* Zero is not just another gridline — it is the floor every
                reading is measured from, so it is drawn solid and in
                `border-strong` while the rest of the grid stays dashed. */}
            <ReferenceLine
              yAxisId="left"
              y={0}
              stroke="var(--color-border-strong)"
              strokeWidth={1}
            />
            <XAxis
              dataKey="timestamp"
              type="number"
              domain={xDomain}
              tickCount={xTickCount}
              tickFormatter={(ts) => formatXTick(ts, timescale)}
              tick={hideXAxis ? false : { fontSize: 11, fontFamily: 'var(--font-mono)', fill: 'var(--color-chart-axis)' }}
              stroke="var(--color-chart-grid)"
              allowDataOverflow
              hide={hideXAxis}
              height={hideXAxis ? 0 : 16}
            />
            <YAxis
              yAxisId="left"
              domain={resolvedYDomain}
              tickFormatter={yFormatter}
              // Explicit ticks only when the axis has a fixed domain worth
              // naming. Under autoscale the ceiling moves every sample, so a
              // hardcoded 0/50/100 would be gridlines drawn where the data no
              // longer is.
              ticks={!autoscale && yTicks ? yTicks : undefined}
              interval={0}
              tick={{ fontSize: 11, fontFamily: 'var(--font-mono)', fill: 'var(--color-chart-axis)' }}
              stroke="transparent"
              tickCount={!autoscale && yTicks ? undefined : 4}
              width={38}
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
                          <span className="text-[11px] text-text-muted tabular-nums">
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
                          <span className="text-[11px] shrink-0" style={{ color: entry.color }}>━</span>
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
                  <stop offset="5%" stopColor={line.color} stopOpacity={0.28} />
                  <stop offset="95%" stopColor={line.color} stopOpacity={0} />
                </linearGradient>
              ))}
            </defs>
            {/*
              Invisible — present only so the window's peak/trough (whatever
              bucket you're currently hovering) reaches the tooltip as a
              plain number. The window's single max/min is named in the
              legend's tier 3; this Area is what makes the *per-bucket*
              extreme available on hover as well.
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
            {lines.map((line) => {
              const tier = line.tier ?? 'derived';
              const style = TIER_STYLE[tier];
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
                  strokeWidth={style.width}
                  strokeOpacity={style.opacity}
                  strokeDasharray={line.dash}
                  // Area belongs to the primary tier alone. Two translucent
                  // fills on one plot stop being two readable series and
                  // become one ambiguous wash — which is the whole reason
                  // `maxPerCard` on the primary tier is 1.
                  //
                  // Still withheld while the M4 trace is drawn: a gradient
                  // under a trend line that the min/max trace keeps crossing
                  // muddles both, and putting extremes on a hairline is only
                  // cheap if nothing is painted behind them.
                  fill={tier === 'primary' && !m4 ? `url(#grad-${line.dataKey})` : 'transparent'}
                  fillOpacity={1}
                  // No per-point dot renderer. Recharts would otherwise mount a
                  // element per sample per series, which is thousands of nodes
                  // to draw a line that is already a path.
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
            {/*
              No end-of-line value pills and no peak/trough badges over the
              plot. Both printed a number the legend already holds, and both
              paid for the repetition in the one currency a 180px cell has
              none of: they sat on the curves. The peak badge in particular
              crossed the line it described.

              The values did not go anywhere — the peak and trough rows are
              tier 3 in the legend above, where they read as a different kind
              of fact from the current value instead of competing with it.
            */}
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
        The card's readout: title, then three ranks, laid over the top of the
        plot so it costs no height.

        It used to be one flat row of identically-sized pills — the current
        value, the P/E core split, the window peak and the window trough all
        rendered at 10px with the same weight, so a card offered six things
        that each claimed to be the answer. Worse, everything past the card's
        width was destroyed silently: Network's six pills measured 617px
        against a 326px card, and the rows that most need a *name* to be
        readable were the first to go.

        Ranking fixes both. One headline in the series colour answers "what is
        it now"; the co-star sits beside it; everything derived — peaks,
        troughs, totals, the core split — is pushed right, muted, and set
        behind a rule, because a peak is a different kind of fact from a
        current value and reading them as one list was the original mistake.
        A phone cell is 186px wide with 38 of that spent on the axis, so only
        the title and the headline fit — the co-star waits for `sm` and the
        third rank for `xl`. Overflowing instead is not an option here: these
        cells share edges, so a legend that runs long does not clip politely,
        it prints itself across the neighbouring chart.

        The third rank appears from `xl` and not before. Below that the space
        left beside the headline is ~117px against the ~273px it wants, and the
        guide's rule for a narrow viewport is remove, never shrink — a clipped
        row is the silent deletion this whole readout was built to stop. The
        peaks it drops are still in the tooltip on hover.
      */}
      <div className="pointer-events-none absolute top-1 left-[38px] right-2 z-10 flex items-baseline gap-2">
        {/* The title yields, never the number. Principle 1 of the guide is
            that chrome is not allowed to outrank data, and on a 186px cell
            something has to give — so the title truncates and the headline
            is `shrink-0`. */}
        <span className="min-w-0 truncate text-[13px] font-medium text-text-secondary">
          {title}
        </span>

        {tier1.map((l) => {
          // The unit rides at caption size, the way the chips already set
          // theirs: "0.058" is the reading and "MB/s" is what it is measured
          // in, and giving both 20px spends a third of a phone cell saying
          // something that never changes.
          const [num, unit] = splitUnit(l.value ?? '—');
          return (
            <span key={`t1-${l.label}`} className="shrink-0 flex items-baseline gap-1">
              <span
                className="font-mono text-[15px] sm:text-[20px] font-bold tabular-nums leading-none"
                style={{ color: l.color }}
              >
                {num}
              </span>
              {unit && (
                <span className="text-[11px] font-mono" style={{ color: l.color }}>{unit}</span>
              )}
              {/* Redundant beside the title on a phone: the card is already
                  called Memory, so the headline does not also need "Used". */}
              <span className="hidden sm:inline text-[11px] font-mono text-text-muted">{l.label}</span>
            </span>
          );
        })}

        {tier2.map((l) => (
          <span key={`t2-${l.label}`} className="shrink-0 hidden sm:flex items-baseline gap-1">
            <span className="font-mono text-[13px] font-medium tabular-nums text-text-primary">
              {l.value ?? '—'}
            </span>
            <span className="text-[11px] font-mono text-text-muted">{l.label}</span>
          </span>
        ))}

        {tier3.length > 0 && (
          <span className="hidden xl:flex ml-auto min-w-0 items-baseline gap-2">
            {/* The rule is the whole point: it says "past here is a different
                kind of number", which is what stops a peak reading as a
                current value. */}
            <span className="shrink-0 w-px h-3 bg-border self-center" aria-hidden />
            {tier3.map((l, idx) => (
              <span key={`t3-${l.label}-${idx}`} className="shrink-0 flex items-baseline gap-1">
                <span className="text-[11px] font-mono text-text-muted">{l.label}</span>
                <span className="text-[11px] font-mono tabular-nums text-text-secondary">
                  {l.value ?? '—'}
                </span>
              </span>
            ))}
          </span>
        )}
      </div>
    </div>
  );
}
