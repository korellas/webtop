import { useYAxisScale, useXAxisScale, usePlotArea } from 'recharts';
import { useHoverStore } from '../store/hover-store';

/** One row of the echo panel — the same series the real tooltip would list. */
export interface EchoSeries {
  dataKey: string;
  color: string;
  label: string;
  /** Plots against the right-hand axis (see `LineConfig.secondary`). */
  secondary?: boolean;
  format?: (v: number) => string;
  /** Draw a dot where this series sits, not just a panel row. Off for the
   *  per-bucket peak/trough carriers: they have no visible curve to mark,
   *  and the real tooltip shows no active dot for them either. */
  dot?: boolean;
}

interface ChartHoverEchoProps {
  /** Identity of the host chart, matched against the hover's owner. */
  chartId: string;
  /** The rows this chart actually plots (post-downsample/smooth). */
  rows: Record<string, unknown>[];
  series: EchoSeries[];
}

/**
 * How far the echo's *readout* is faded from the real tooltip. Low enough that
 * the chart under the pointer is unmistakably the one being read, high enough
 * that the numbers on the other five are legible without hunting for them.
 */
const ECHO_OPACITY = 0.55;

/**
 * The cursor line is not faded with the readout — it is drawn at exactly the
 * strength of Recharts' own cursor on the hovered card.
 *
 * The position is the one thing that is genuinely identical across all six
 * cards; it is the readout, not the mark, that needs to say "you are not
 * pointing here". Fading the line as well multiplied 0.6 by the group's 0.55
 * and left a mid-grey dash at a third of its intended contrast — invisible on
 * a busy plot, which is where you most need to know where you are.
 */
const CURSOR_OPACITY = 0.6;

const DOT_R = 2.5;
/** Host box for the panel. The panel itself hugs its content inside this
 *  box (aligned to whichever edge faces the cursor line), so the box only
 *  has to be an upper bound, not a measurement. */
const PANEL_BOX_W = 150;
const PANEL_GAP = 8;
const ROW_H = 15;
const PANEL_CHROME_H = 16;

/**
 * The row nearest a timestamp.
 *
 * Every card buckets the same source rows at the same timescale, so the
 * timestamps normally match exactly — but a card that derives its own series
 * (the Temperature card rebuilds each row) has no guarantee of that, and an
 * exact-match lookup would silently drop the echo there. Nearest-wins costs a
 * linear scan over at most `MAX_POINTS` rows.
 */
function nearestRow(
  rows: Record<string, unknown>[],
  timestamp: number,
): Record<string, unknown> | undefined {
  let best: Record<string, unknown> | undefined;
  let bestDist = Infinity;
  for (const row of rows) {
    const t = row.timestamp;
    if (typeof t !== 'number') continue;
    const dist = Math.abs(t - timestamp);
    if (dist < bestDist) {
      bestDist = dist;
      best = row;
    }
  }
  return best;
}

/**
 * A faded copy of the hover readout, drawn on every chart *except* the one
 * under the pointer: a cursor line at the shared timestamp, a dot where each
 * series sits there, and the values it held.
 *
 * Values only — no clock header. Every echo is at the same instant by
 * construction, so five repeats of one timestamp is five panels' worth of
 * height and ink saying nothing the tooltip under the pointer doesn't already
 * say. It also kept colliding with the peak-marker pills that live along the
 * plot's top edge, and a half-covered timestamp reads as a bug.
 *
 * Rendered as a chart child (not an absolutely-positioned sibling) because
 * the value→pixel scales only exist inside Recharts' own context — the same
 * reason `ExtremeMarkers` lives there. The `data-overlay` marker opts it into
 * MetricChart's layout effect that reorders overlays to paint above the
 * curves.
 *
 * It reads the hover itself rather than taking it as a prop, and that is the
 * whole performance story: subscribing in `MetricChart` re-rendered six entire
 * AreaCharts per pointer move. Subscribing here re-renders six of *these*.
 */
export default function ChartHoverEcho({ chartId, rows, series }: ChartHoverEchoProps) {
  const timestamp = useHoverStore((s) => (s.sourceId === chartId ? null : s.timestamp));
  const yLeft = useYAxisScale('left');
  const yRight = useYAxisScale('right');
  const xScale = useXAxisScale();
  const plot = usePlotArea();
  // No echo on the chart under the pointer: it already draws Recharts' own
  // cursor and tooltip, and doubling them is the same readout twice.
  if (timestamp === null || !plot) return null;

  const row = nearestRow(rows, timestamp);
  if (!row) return null;

  const rowTs = row.timestamp as number;
  const x = xScale?.(rowTs);
  if (typeof x !== 'number' || !Number.isFinite(x)) return null;
  // The hovered timestamp can sit outside this chart's plotted range while a
  // window is still filling up. Drawing the line clamped to an edge would
  // claim a reading that isn't there.
  if (x < plot.x || x > plot.x + plot.width) return null;

  const points = series
    .map((s) => {
      const v = row[s.dataKey];
      if (typeof v !== 'number' || !Number.isFinite(v)) return undefined;
      const y = (s.secondary ? yRight : yLeft)?.(v);
      if (typeof y !== 'number' || !Number.isFinite(y)) return undefined;
      return { ...s, value: v, y };
    })
    .filter((p): p is EchoSeries & { value: number; y: number } => p !== undefined);
  if (points.length === 0) return null;

  // Panel on whichever side of the cursor line has room, hugging that edge —
  // the same flip Recharts does for the real tooltip, so the echo lands where
  // the eye already expects a readout.
  const roomRight = plot.x + plot.width - x;
  const toRight = roomRight >= PANEL_BOX_W + PANEL_GAP;
  const boxX = toRight ? x + PANEL_GAP : x - PANEL_GAP - PANEL_BOX_W;
  const boxH = PANEL_CHROME_H + points.length * ROW_H;

  return (
    <g data-overlay="hover-echo" style={{ pointerEvents: 'none' }}>
      <line
        x1={x}
        x2={x}
        y1={plot.y}
        y2={plot.y + plot.height}
        stroke="var(--color-chart-axis)"
        strokeWidth={1}
        strokeDasharray="3 3"
        strokeOpacity={CURSOR_OPACITY}
      />
      <g opacity={ECHO_OPACITY}>
      {points.filter((p) => p.dot !== false).map((p) => (
        <circle
          key={p.dataKey}
          cx={x}
          cy={p.y}
          r={DOT_R}
          fill={p.color}
          stroke="var(--color-bg-card)"
          strokeWidth={1}
        />
      ))}
      <foreignObject
        x={boxX}
        y={plot.y + 2}
        width={PANEL_BOX_W}
        height={boxH}
        style={{ overflow: 'visible' }}
      >
        <div className={`flex ${toRight ? 'justify-start' : 'justify-end'}`}>
          <div
            style={{ background: 'var(--color-bg-hover)', boxShadow: '0 8px 24px rgb(0 0 0 / 0.45)' }}
            className="border border-border-strong rounded-lg px-2.5 py-2 flex flex-col gap-1"
          >
            {points.map((p) => (
              <div key={p.dataKey} className="flex items-center gap-2 text-[11px] leading-none">
                <span className="text-[10px] shrink-0" style={{ color: p.color }}>━</span>
                <span className="text-text-secondary">{p.label}</span>
                <span className="font-semibold tabular-nums text-text-primary ml-auto pl-3">
                  {p.format ? p.format(p.value) : p.value.toFixed(2)}
                </span>
              </div>
            ))}
          </div>
        </div>
      </foreignObject>
      </g>
    </g>
  );
}
