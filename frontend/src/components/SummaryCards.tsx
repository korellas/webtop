import { useMetricsStore } from '../store/metrics-store';
import { useSystemStore } from '../store/system-store';
import { useDrawerStore, type CardKey, type AnchorPos } from '../store/drawer-store';
import { formatGB, formatWatts, formatWh, formatMBps, formatPercent } from '../lib/format';
import { COLORS } from '../lib/colors';
import RingGauge from './RingGauge';

/** 6 GB/s — approximate NVMe peak for a modern Apple Silicon Mac. */
const DISK_IO_MAX_BYTES = 6 * 1024 * 1024 * 1024;

/** Watts without unit suffix — for compact sub-line use. */
const fmtW = (w: number) => (w < 10 ? w.toFixed(1) : String(Math.round(w)));

/** Capture the anchor position from a click so the dropdown can point to this chip. */
function captureAnchor(e: React.MouseEvent<HTMLButtonElement>): AnchorPos {
  const rect = e.currentTarget.getBoundingClientRect();
  return { x: rect.left + rect.width / 2, bottom: rect.bottom + 6 };
}

export default function SummaryCards() {
  const latest = useMetricsStore((s) => s.snapshots[s.snapshots.length - 1]);
  const info = useSystemStore((s) => s.info);

  if (!latest || !info) {
    return <div className="h-12 shrink-0" />;
  }

  const cpuTotal = latest.cpu_total;
  const memPct = (latest.mem_used / info.mem_total) * 100;
  const diskPct = info.disk_total > 0 ? (latest.disk_used / info.disk_total) * 100 : 0;
  const powerPct = (latest.power_total_w / 400) * 100;

  const linkSpeed = info.net_link_speed_bytes_sec ?? 125_000_000;
  const netTotalBytes = latest.net_up_bytes_sec + latest.net_down_bytes_sec;
  const netPct = Math.min((netTotalBytes / linkSpeed) * 100, 100);

  const diskIoPct = Math.min(
    (Math.max(latest.disk_read_bytes_sec, latest.disk_write_bytes_sec) / DISK_IO_MAX_BYTES) * 100,
    100,
  );

  const energyPct = Math.min((latest.energy_session_wh / 5000) * 100, 100);
  const prevMonthWh = latest.energy_prev_month_wh ?? 0;

  return (
    <div
      /*
        The strip scrolls below `lg`, where the chips cannot all fit. It used
        to scroll with `no-scrollbar` and no other mark, so the last visible
        chip was sliced off at a hard edge and everything behind it was
        undiscoverable: a cut card reads as a layout bug, not as an invitation.
        The right-edge fade is the standard affordance and costs no height,
        which this layout has none of to give. From `lg` up the chips are
        `flex-1` and fill the row, so there is nothing to fade.
      */
      className="
        flex gap-1.5 overflow-x-auto no-scrollbar shrink-0 pb-0.5
        [mask-image:linear-gradient(to_right,black_calc(100%-24px),transparent)]
        lg:[mask-image:none]
      "
    >
      <MiniChip
        card="cpu"
        label="CPU"
        value={formatPercent(cpuTotal)}
        sub={`P${formatPercent(latest.cpu_p_cores)} E${formatPercent(latest.cpu_e_cores)}`}
        gauge={<RingGauge value={cpuTotal} color={COLORS.compute} size={30} strokeWidth={2.5} />}
      />
      {/* GPU — non-clickable: no drill-down view worth showing (unified memory, no VRAM). */}
      <MiniChip
        label="GPU"
        value={formatPercent(latest.gpu_usage)}
        sub={`${info.gpu_core_count}-core`}
        gauge={<RingGauge value={latest.gpu_usage} color={COLORS.gpu} size={30} strokeWidth={2.5} />}
      />
      <MiniChip
        card="ram"
        label="RAM"
        value={formatPercent(memPct)}
        sub={formatGB(latest.mem_used, 1)}
        severity={severity(memPct, THRESHOLD.mem)}
        gauge={<RingGauge value={memPct} color={COLORS.memory} size={30} strokeWidth={2.5} />}
      />
      <MiniChip
        card="disk"
        label="Disk"
        value={`${diskPct.toFixed(0)}%`}
        sub={formatGB(latest.disk_used, 0)}
        gauge={<RingGauge value={diskPct} color={COLORS.storage} size={30} strokeWidth={2.5} />}
      />
      <MiniDualChip
        card="net"
        label="Net"
        gauge={<RingGauge value={netPct} color={COLORS.network} size={30} strokeWidth={2.5} />}
        rows={[
          { icon: '▲', color: COLORS.networkLight, value: formatMBps(latest.net_up_bytes_sec) },
          { icon: '▼', color: COLORS.network, value: formatMBps(latest.net_down_bytes_sec) },
        ]}
      />
      {/* I/O — non-clickable: drilling down would duplicate the Disk drawer. */}
      <MiniDualChip
        label="I/O"
        gauge={<RingGauge value={diskIoPct} color={COLORS.storage} size={30} strokeWidth={2.5} />}
        rows={[
          { icon: 'R', color: COLORS.storage, value: formatMBps(latest.disk_read_bytes_sec) },
          { icon: 'W', color: COLORS.storageLight, value: formatMBps(latest.disk_write_bytes_sec) },
        ]}
      />
      <MiniChip
        card="power"
        label="Power"
        value={formatWatts(latest.power_total_w)}
        // Prefixed, like the CPU chip's `P3% E18%`. Three bare numbers under a
        // chip labelled "Power" name nothing: `5.2 54 100` could be anything.
        sub={`C${fmtW(latest.power_cpu_w)} G${fmtW(latest.power_gpu_w)} O${fmtW(latest.power_other_w)}`}
        gauge={<RingGauge value={powerPct} color={COLORS.power} size={30} strokeWidth={2.5} />}
      />
      <MiniChip
        card="energy"
        label="Energy"
        value={formatWh(latest.energy_session_wh)}
        sub={prevMonthWh > 0 ? `prev ${formatWh(prevMonthWh)}` : 'this month'}
        gauge={<RingGauge value={energyPct} color={COLORS.power} size={30} strokeWidth={2.5} />}
      />
    </div>
  );
}

// ─── Sub-components ──────────────────────────────────────────────────────────

/**
 * Which chart each chip is about.
 *
 * Six cells and eight chips, so this is many-to-one: GPU rides the CPU card,
 * ENERGY rides Power, and both disk chips point at Disk I/O.
 */
const CHIP_CHART: Record<string, string> = {
  CPU: 'cpu-gpu',
  GPU: 'cpu-gpu',
  RAM: 'mem',
  Disk: 'disk',
  'I/O': 'disk',
  Net: 'net',
  Power: 'power',
  Energy: 'power',
};

/**
 * Take the phone's chart column to the card a chip is about.
 *
 * Six cells at 180px is a 1080px scroll on a phone, and the chip strip sits
 * above all of it naming things the reader cannot see — so a chip that reports
 * a number and cannot take you to its history is an index with no page
 * numbers. Interactive chips still open their drawer on top; this just means
 * that when the drawer is dismissed, the chart it was about is the one on
 * screen.
 *
 * A no-op on a desktop, where every cell is already visible and
 * `scrollIntoView` would move nothing.
 */
function scrollToChart(label: string) {
  const key = CHIP_CHART[label];
  if (!key) return;
  const cell = document.querySelector(`[data-chart="${key}"]`);
  cell?.scrollIntoView({
    behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth',
    block: 'start',
  });
}

/**
 * Shared class names for both the clickable and non-clickable chip variants
 * so they stay visually identical apart from the interactive affordances.
 */
const CHIP_BASE =
  'flex items-center justify-between gap-2 px-3 py-2 bg-bg-card border rounded-card flex-1 min-w-[160px]';
// 152, and it is the sub-line that sets it. The gauge (30), the gaps (8) and
// `px-3` (24) come off the top, so the text column gets whatever is left, and
// the longest sub — `prev 18.43 kWh` — measures 92px at the 11px mono the type
// scale gives it. design-guide.md §3 fixes this at
// 144 on the basis of an 86px
// sub, which is that same string at 10px sans; 144 truncated the energy and
// power chips at every width below 1512. Raised rather than shrinking the type,
// because the scale is the part with an accessibility floor under it.
//
// 160 and not 152, which is what the arithmetic alone gives: the two longest
// subs are `prev 18.43 kWh` and a three-digit power split (`C136 G136 O136`),
// both 14 characters, both 92px at 0.6em advance. Sizing the box to exactly
// that leaves a boundary the live data crosses and re-crosses, so the chip
// truncated intermittently rather than never. The extra 8px is the margin
// that turns "fits today" into "fits".
/**
 * Thresholds, from the token sheet. A chip past one changes its *border* and
 * nothing else.
 *
 * Not a filled background and emphatically not a blink. This screen redraws
 * twice a second and is meant to be left open on a second monitor; something
 * flashing in the corner of the eye is not information, it is a thing you
 * learn to stop seeing. A border is legible at a glance, survives peripheral
 * vision, and stays out of the way of the number it is about.
 *
 * Colour never carries the meaning alone — the value it qualifies is right
 * there, in figures, at 15px.
 */
export type Severity = 'ok' | 'warn' | 'crit';

const THRESHOLD = {
  temp: { warn: 85, crit: 95 },   /* °C  */
  mem: { warn: 85, crit: 95 },    /* % of total */
} as const;

function severity(value: number | null | undefined, at: { warn: number; crit: number }): Severity {
  if (value == null || Number.isNaN(value)) return 'ok';
  if (value >= at.crit) return 'crit';
  if (value >= at.warn) return 'warn';
  return 'ok';
}

const SEVERITY_BORDER: Record<Severity, string> = {
  ok: 'border-border',
  warn: 'border-warning',
  crit: 'border-danger',
};

/**
 * Every chip's class string, composed in one place.
 *
 * `CHIP_BASE` carries `border` without a colour so that severity can supply
 * one — which means a chip assembled without it gets `currentColor`, and in
 * light mode that is #09090b. The Net chip shipped exactly that: one hard
 * black outline in a row of hairlines, because a single branch of the four
 * was missed. Tailwind cannot rescue this with an override either, since two
 * utilities for the same property resolve by CSS source order and not by the
 * order they appear in the attribute.
 *
 * So there is no longer a way to build a chip and forget: the border is not
 * a class anyone appends, it is a parameter of this function.
 */
function chipClass(sev: Severity, opts?: { interactive?: boolean; extra?: string }) {
  return [
    CHIP_BASE,
    SEVERITY_BORDER[sev],
    opts?.interactive ? CHIP_INTERACTIVE : '',
    'text-left',
    opts?.extra ?? '',
  ].filter(Boolean).join(' ');
}

const CHIP_INTERACTIVE =
  'hover:bg-bg-hover hover:border-border-strong active:scale-[0.98] transition-[background-color,border-color,transform] duration-150 cursor-pointer focus:outline-none focus-visible:ring-2 focus-visible:ring-border-strong';

function MiniChip({
  card,
  label,
  value,
  sub,
  gauge,
  severity: sev = 'ok',
}: {
  /** Omit to render as a non-clickable display chip. */
  card?: CardKey;
  label: string;
  value: string;
  sub?: string;
  gauge?: React.ReactNode;
  severity?: Severity;
}) {
  const toggle = useDrawerStore((s) => s.toggle);
  const openCard = useDrawerStore((s) => s.openCard);

  const content = (
    <>
      {gauge}
      <div className="text-right min-w-0">
        <div className="font-mono text-[11px] uppercase tracking-[.08em] text-text-secondary leading-none mb-1">
          {label}
        </div>
        <div className="font-mono text-[15px] font-semibold tabular-nums leading-tight">{value}</div>
        {sub && (
          <div
            title={sub}
            className="font-mono text-[11px] text-text-muted tabular-nums leading-tight truncate"
          >
            {sub}
          </div>
        )}
      </div>
    </>
  );

  if (!card) {
    // No drawer worth opening, but on a phone it can still be the index entry
    // for its chart — which is the whole of what it can usefully do there.
    return (
      <button
        type="button"
        aria-label={`Scroll to ${label} chart`}
        onClick={() => scrollToChart(label)}
        className={chipClass(sev, { extra: 'md:cursor-default' })}
      >
        {content}
      </button>
    );
  }

  const isActive = openCard === card;
  return (
    <button
      type="button"
      aria-label={`Open ${label} detail`}
      aria-expanded={isActive}
      onClick={(e) => { scrollToChart(label); toggle(card, captureAnchor(e)); }}
      className={chipClass(sev, {
        interactive: true,
        extra: isActive ? 'bg-bg-hover border-border-strong' : '',
      })}
    >
      {content}
    </button>
  );
}

function MiniDualChip({
  card,
  label,
  rows,
  gauge,
}: {
  card?: CardKey;
  label: string;
  rows: Array<{ icon: string; color: string; value: string }>;
  gauge?: React.ReactNode;
}) {
  const toggle = useDrawerStore((s) => s.toggle);
  const openCard = useDrawerStore((s) => s.openCard);

  const content = (
    <>
      {gauge}
      <div className="text-right">
        <div className="font-mono text-[11px] uppercase tracking-[.08em] text-text-secondary leading-none mb-1">
          {label}
        </div>
        {rows.map((row, i) => {
          // Split "1.300 MB/s" → num="1.300" unit="MB/s" so the unit never shifts
          const spaceIdx = row.value.indexOf(' ');
          const num = spaceIdx >= 0 ? row.value.slice(0, spaceIdx) : row.value;
          const unit = spaceIdx >= 0 ? row.value.slice(spaceIdx + 1) : '';
          return (
            <div key={i} className="flex items-center justify-end gap-0.5">
              <span
                className="font-mono text-[11px] font-semibold w-2.5 text-center shrink-0"
                style={{ color: row.color }}
              >
                {row.icon}
              </span>
              <span className="font-mono text-[13px] font-semibold tabular-nums leading-tight inline-block text-right min-w-[3.2em]">
                {num}
              </span>
              <span className="text-[11px] text-text-secondary leading-tight shrink-0">{unit}</span>
            </div>
          );
        })}
      </div>
    </>
  );

  // Neither of this variant's metrics is a proportion of a ceiling, so there
  // is nothing here for a threshold to be crossed.
  const sev: Severity = 'ok';

  if (!card) {
    // No drawer worth opening, but on a phone it can still be the index entry
    // for its chart — which is the whole of what it can usefully do there.
    return (
      <button
        type="button"
        aria-label={`Scroll to ${label} chart`}
        onClick={() => scrollToChart(label)}
        className={chipClass(sev, { extra: 'md:cursor-default' })}
      >
        {content}
      </button>
    );
  }

  const isActive = openCard === card;
  return (
    <button
      type="button"
      aria-label={`Open ${label} detail`}
      aria-expanded={isActive}
      onClick={(e) => { scrollToChart(label); toggle(card, captureAnchor(e)); }}
      className={chipClass(sev, {
        interactive: true,
        extra: isActive ? 'bg-bg-hover border-border-strong' : '',
      })}
    >
      {content}
    </button>
  );
}
