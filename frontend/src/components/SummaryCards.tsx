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
 * Shared class names for both the clickable and non-clickable chip variants
 * so they stay visually identical apart from the interactive affordances.
 */
const CHIP_BASE =
  'flex items-center justify-between gap-2 px-3 py-2 bg-bg-card border rounded-card flex-1 min-w-[144px]';
// 144, not 108: the 30 px gauge, the gaps and `px-3` leave the text column
// whatever remains, and at 108 that was 47 px — enough for the value but not
// for any sub-line, so `P2% E22%`, the power split and `prev 18.43 kWh` were
// all truncated at every width below `lg`. The widest sub measures 77 px.
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

  const border = SEVERITY_BORDER[sev];

  if (!card) {
    return <div className={`${CHIP_BASE} ${border} text-left`}>{content}</div>;
  }

  const isActive = openCard === card;
  return (
    <button
      type="button"
      aria-label={`Open ${label} detail`}
      aria-expanded={isActive}
      onClick={(e) => toggle(card, captureAnchor(e))}
      className={`${CHIP_BASE} ${border} ${CHIP_INTERACTIVE} text-left ${
        isActive ? 'bg-bg-hover border-border-strong' : ''
      }`}
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
              <span className="text-[9px] text-text-secondary leading-tight shrink-0">{unit}</span>
            </div>
          );
        })}
      </div>
    </>
  );

  // Neither of this variant's metrics is a proportion of a ceiling, so
  // there is nothing here for a threshold to be crossed.
  const border = SEVERITY_BORDER.ok;

  if (!card) {
    return <div className={`${CHIP_BASE} ${border} text-left`}>{content}</div>;
  }

  const isActive = openCard === card;
  return (
    <button
      type="button"
      aria-label={`Open ${label} detail`}
      aria-expanded={isActive}
      onClick={(e) => toggle(card, captureAnchor(e))}
      className={`${CHIP_BASE} ${CHIP_INTERACTIVE} text-left ${
        isActive ? 'bg-bg-hover border-border-strong' : ''
      }`}
    >
      {content}
    </button>
  );
}
