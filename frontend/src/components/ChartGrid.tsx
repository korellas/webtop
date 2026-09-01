import { useMemo } from 'react';
import { useMetricsStore } from '../store/metrics-store';
import { useSystemStore } from '../store/system-store';
import { COLORS } from '../lib/colors';
import { formatBytes, formatMBps, formatWatts, formatGB } from '../lib/format';
import { attachBands, BAND_LO, BAND_HI } from '../lib/chart-utils';
import MetricChart from './MetricChart';

type ChartData = Record<string, unknown>[];

/**
 * Add a unit-converted copy of a banded series, bounds included.
 *
 * Charts that plot GB or MB/s rather than raw bytes need the band converted
 * too — a band left in bytes would be plotted against a GB axis and shoot off
 * the top of the chart. Keeping the conversion in one helper means the two
 * bounds can never drift from the value they bracket.
 */
function derive(
  row: Record<string, unknown>,
  from: string,
  to: string,
  divisor: number,
): void {
  // Guard against a sensor that doesn't report every tick (fan RPM is
  // idle-gated on some Macs, like GPU temp) — `undefined / divisor` is
  // `NaN`, which breaks the SVG path outright instead of leaving a gap
  // `connectNulls` can bridge. `null` reproduces the pass-through
  // behavior every other derived series already had.
  const v = row[from];
  row[to] = typeof v === 'number' ? v / divisor : null;
  const lo = row[`${from}${BAND_LO}`];
  const hi = row[`${from}${BAND_HI}`];
  if (typeof lo === 'number') row[`${to}${BAND_LO}`] = lo / divisor;
  if (typeof hi === 'number') row[`${to}${BAND_HI}`] = hi / divisor;
}

const GB = 1024 ** 3;
const MB = 1024 * 1024;

// Mac Studio fans top out higher (≈3625 RPM per F0Mx), but the firmware
// rarely drives them above 3000 even under sustained load, so this stays
// a useful "% of typical max" scale instead of always hugging the bottom.
const FAN_MAX_RPM = 3000;

function pct(v: number | undefined): string {
  if (v === undefined || Number.isNaN(v)) return '—';
  return v < 10 ? `${v.toFixed(1)}%` : `${Math.round(v)}%`;
}

/**
 * Y-axis ticks and peak labels for the byte-rate charts, whose input is MB/s.
 *
 * Every branch is bounded at five characters, because the axis gutter is 38 px
 * and silently truncates from the left — which does not produce an ugly label,
 * it produces a *wrong* one. A 7-day disk peak of 7373 MB/s formatted as
 * `7373.0M` and rendered as `00.0M`; the first repair, keeping one decimal up
 * to 1024, formatted a 188 MB/s peak's ticks as `195.0M` and rendered `95.0M`,
 * an axis that appeared to count 95, 30, 65, 0 downward.
 *
 * So the decimal is spent only where it carries information, the same rule
 * `format.ts` uses throughout: below 10, where the integer alone would round a
 * reading away. `MetricChart` labels the peak legend rows with this formatter
 * too, so the chart and its legend read in one unit system.
 */
function formatRateTick(v: number): string {
  if (v < 1) return `${(v * 1024).toFixed(0)}K`;
  if (v < 10) return `${v.toFixed(1)}M`;
  if (v < 1024) return `${Math.round(v)}M`;
  return `${(v / 1024).toFixed(1)}G`;
}

export default function ChartGrid() {
  const snapshots = useMetricsStore((s) => s.snapshots);
  const networkTotals = useMetricsStore((s) => s.networkTotals);
  const info = useSystemStore((s) => s.info);

  const memTotalGB = info ? info.mem_total / GB : 76;
  const latest = snapshots[snapshots.length - 1];
  const memPct = info && latest && info.mem_total > 0
    ? (latest.mem_used / info.mem_total) * 100
    : undefined;

  // Flatten each row's [min, max] bounds into scalar `_lo`/`_hi` keys once,
  // here, so every chart below shares the work and `downsample` sees a flat
  // record (it assumes scalars and would carry a nested object through
  // untouched, silently dropping the range).
  // Memoized on `snapshots`: this copies every retained sample (up to the
  // 3 600-entry buffer) and the four per-card derivations below copy them
  // again. Recomputing that for a re-render the data did not cause — a theme
  // flip, a panel opening — is pure waste.
  const banded = useMemo(() => attachBands(snapshots), [snapshots]);

  const tempData = useMemo(() => banded.map((r) => {
    const row = { ...r };
    derive(row, 'fan_rpm', 'fan_pct', FAN_MAX_RPM / 100);
    return row;
  }) as ChartData, [banded]);

  const memData = useMemo(() => banded.map((r) => {
    const row = { ...r };
    derive(row, 'mem_used', 'mem_used_gb', GB);
    row.mem_swap_gb = (row.mem_swap_used as number) / GB;
    return row;
  }) as ChartData, [banded]);

  const netData = useMemo(() => banded.map((r) => {
    const row = { ...r };
    derive(row, 'net_up_bytes_sec', 'net_up_mbs', MB);
    derive(row, 'net_down_bytes_sec', 'net_down_mbs', MB);
    return row;
  }) as ChartData, [banded]);

  const diskData = useMemo(() => banded.map((r) => {
    const row = { ...r };
    derive(row, 'disk_read_bytes_sec', 'disk_read_mbs', MB);
    derive(row, 'disk_write_bytes_sec', 'disk_write_mbs', MB);
    return row;
  }) as ChartData, [banded]);

  // Whether this machine ever reported a non-zero CPU die temperature.
  // Intel Macs report 0 forever; Apple Silicon needs a few seconds for
  // the first sensor read after cold-boot. We hide the temperature card
  // entirely until at least one real reading arrives.
  //
  // GPU temp deliberately *not* shown — on M3 / M3 Ultra / M4 the `Tg*`
  // SMC keys are idle-gated and only populate under sustained load, so
  // a "GPU temp" line is misleading 90 %+ of the time. CPU die temp is
  // a fine proxy for system thermal state since CPU/GPU share the same
  // SoC die. See https://github.com/vladkens/macmon/issues/12 for the
  // upstream confirmation of the M3/M4 sensor gating.
  const hasTemp = snapshots.some((s) => (s.cpu_temp_c ?? 0) > 0);
  // Fanless Macs (e.g. MacBook Air) report 0 RPM forever; only show the
  // overlay when at least one sample carries real fan data.
  const hasFan = snapshots.some((s) => (s.fan_rpm ?? 0) > 0);

  const tempFormat = (v: number) => `${Math.round(v)}°`;

  const rows: Array<React.ReactNode[]> = [
    [
      // CPU + GPU share one card now — both are utilisation-percent
      // metrics on the same 0..100 scale, and the user spends most of
      // their hover time correlating them anyway. Merging them frees a
      // grid slot for the new Temperature card without breaking the
      // 3×2 layout.
      <MetricChart
        key="cpu-gpu"
        title="CPU / GPU"
        data={banded as ChartData}
        lines={[
          { dataKey: 'cpu_total', color: COLORS.cpu },
          { dataKey: 'gpu_usage', color: COLORS.gpu },
          { dataKey: 'cpu_p_cores', color: COLORS.cpuP, dashed: true },
          { dataKey: 'cpu_e_cores', color: COLORS.cpuE, dashed: true },
        ]}
        yDomain={[0, 100]}
        yFormatter={(v) => `${Math.round(v)}%`}
        legend={[
          { label: 'CPU', color: COLORS.cpu, value: pct(latest?.cpu_total), dataKey: 'cpu_total', primary: true },
          { label: 'GPU', color: COLORS.gpu, value: pct(latest?.gpu_usage), dataKey: 'gpu_usage', primary: true },
          { label: 'P', color: COLORS.cpuP, value: pct(latest?.cpu_p_cores), dataKey: 'cpu_p_cores' },
          { label: 'E', color: COLORS.cpuE, value: pct(latest?.cpu_e_cores), dataKey: 'cpu_e_cores' },
        ]}
        hideXAxis
      />,
      hasTemp ? (
        <MetricChart
          key="temp"
          title="Temperature"
          // Fan RPM re-expressed as % of FAN_MAX_RPM so it can share CPU
          // temp's 0–100 axis instead of needing a secondary one — this
          // was the only chart of the six with a second axis, which made
          // it look structurally different from the rest of the grid.
          // The axis gridlines are honestly ambiguous now (a reading of
          // "40" is 40°C for one line and 40% of max fan for the other),
          // but `formatter` below keeps the legend, tooltip and end-tag
          // showing real RPM throughout — only the plotted shape and the
          // axis ticks are relative.
          data={tempData}
          lines={[
            // Trough marker enabled here: a sudden temperature drop
            // (throttle recovery) is real signal, unlike CPU/GPU/power/
            // network where the minimum sits near zero and says nothing.
            { dataKey: 'cpu_temp_c', color: COLORS.tempCpu, trough: true },
            // Trough marker here catches a fan spinning down.
            ...(hasFan
              ? [{
                  dataKey: 'fan_pct',
                  color: COLORS.fan,
                  dashed: true,
                  trough: true,
                  formatter: (v: number) => `${Math.round((v * FAN_MAX_RPM) / 100)} RPM`,
                }]
              : []),
          ]}
          // Fixed 0–100 primary axis. Apple Silicon throttles somewhere in
          // the 95–105 °C range depending on chip, so 100 gives a stable
          // visual reference for "approaching thermal limit" without the
          // curve jumping around as the autoscale bounds change every
          // tick — and it's the same 0–100 scale fan_pct is defined on.
          yDomain={[0, 100]}
          yFormatter={tempFormat}
          legend={[
            {
              label: 'CPU',
              color: COLORS.tempCpu,
              value:
                typeof latest?.cpu_temp_c === 'number' && latest.cpu_temp_c > 0
                  ? `${Math.round(latest.cpu_temp_c)}°C`
                  : '—',
              dataKey: 'cpu_temp_c',
              primary: true,
            },
            ...(hasFan
              ? [{
                  label: 'Fan',
                  color: COLORS.fan,
                  value:
                    typeof latest?.fan_rpm === 'number' && latest.fan_rpm > 0
                      ? `${Math.round(latest.fan_rpm)} RPM`
                      : 'idle',
                  dataKey: 'fan_pct',
                }]
              : []),
          ]}
          hideXAxis
        />
      ) : (
        // Intel Macs / sensorless boots: keep the cell so the grid
        // stays balanced, and tell the user why it's empty.
        <div
          key="temp-na"
          className="h-full w-full flex flex-col items-center justify-center gap-1 text-center px-4"
        >
          <span className="text-[11px] font-semibold text-text-primary/90 tracking-wide">
            Temperature
          </span>
          <span className="text-[10px] text-text-muted leading-snug">
            Not available — Apple Silicon only
          </span>
        </div>
      ),
    ],
    [
      <MetricChart
        key="mem"
        title="Memory"
        data={memData}
        lines={[
          { dataKey: 'mem_used_gb', color: COLORS.memory },
          { dataKey: 'mem_swap_gb', color: COLORS.memorySwap, dashed: true },
        ]}
        yDomain={[0, Math.ceil(memTotalGB)]}
        yFormatter={(v) => v < 10 ? `${v.toFixed(1)}G` : `${Math.round(v)}G`}
        legend={[
          {
            label: 'Used',
            color: COLORS.memory,
            value: latest ? formatGB(latest.mem_used) : '—',
            dataKey: 'mem_used_gb',
            primary: true,
          },
          {
            // Not primary, so it rides the title-row pills like P/E cores
            // do on the CPU/GPU card — the end-tag already shows the GB
            // figure; this puts the proportion of total RAM next to it
            // without crowding the tag itself.
            label: 'RAM',
            color: COLORS.memory,
            value: pct(memPct),
          },
          {
            label: 'Swap',
            color: COLORS.memorySwap,
            value: latest ? formatGB(latest.mem_swap_used, 2) : '—',
            dataKey: 'mem_swap_gb',
          },
        ]}
        hideXAxis
      />,
      <MetricChart
        key="power"
        title="Power"
        data={banded as ChartData}
        lines={[
          { dataKey: 'power_total_w', color: COLORS.power },
          { dataKey: 'power_cpu_w', color: COLORS.powerCpu, dashed: true },
          { dataKey: 'power_gpu_w', color: COLORS.powerGpu, dashed: true },
          { dataKey: 'power_other_w', color: COLORS.powerOther, dashed: true },
        ]}
        yDomain={[0, 'auto'] as [number, number | 'auto']}
        yFormatter={(v) => `${Math.round(v)}W`}
        legend={[
          {
            label: 'Total',
            color: COLORS.power,
            value: latest ? formatWatts(latest.power_total_w) : '—',
            dataKey: 'power_total_w',
            primary: true,
          },
          {
            label: 'CPU',
            color: COLORS.powerCpu,
            value: latest ? formatWatts(latest.power_cpu_w) : '—',
            dataKey: 'power_cpu_w',
          },
          {
            label: 'GPU',
            color: COLORS.powerGpu,
            value: latest ? formatWatts(latest.power_gpu_w) : '—',
            dataKey: 'power_gpu_w',
          },
          {
            label: 'Other',
            color: COLORS.powerOther,
            value: latest ? formatWatts(latest.power_other_w) : '—',
            dataKey: 'power_other_w',
          },
        ]}
        hideXAxis
      />,
    ],
    [
      <MetricChart
        key="net"
        title="Network"
        data={netData}
        lines={[
          { dataKey: 'net_up_mbs', color: COLORS.networkUp },
          { dataKey: 'net_down_mbs', color: COLORS.networkDown },
        ]}
        yDomain={[0, 'auto'] as [number, number | 'auto']}
        yFormatter={formatRateTick}
        legend={[
          {
            label: '▲',
            color: COLORS.networkUp,
            value: latest ? formatMBps(latest.net_up_bytes_sec) : '—',
            dataKey: 'net_up_mbs',
            primary: true,
          },
          {
            label: '▼',
            color: COLORS.networkDown,
            value: latest ? formatMBps(latest.net_down_bytes_sec) : '—',
            dataKey: 'net_down_mbs',
            primary: true,
          },
          {
            // Total transferred over the visible window — an integral of
            // the rate curve above it, not another rate. Not `primary`: it
            // rides the title-row pills like P/E cores do, since the ▲/▼
            // rates already own the hero end-tags.
            label: 'Σ▲',
            color: COLORS.networkUp,
            value: networkTotals ? formatBytes(networkTotals.up_bytes) : '—',
          },
          {
            label: 'Σ▼',
            color: COLORS.networkDown,
            value: networkTotals ? formatBytes(networkTotals.down_bytes) : '—',
          },
        ]}
      />,
      <MetricChart
        key="disk"
        title="Disk I/O"
        data={diskData}
        lines={[
          { dataKey: 'disk_read_mbs', color: COLORS.disk },
          { dataKey: 'disk_write_mbs', color: COLORS.diskWrite },
        ]}
        yDomain={[0, 'auto'] as [number, number | 'auto']}
        yFormatter={formatRateTick}
        legend={[
          {
            label: 'Read',
            color: COLORS.disk,
            value: latest ? formatMBps(latest.disk_read_bytes_sec) : '—',
            dataKey: 'disk_read_mbs',
            primary: true,
          },
          {
            label: 'Write',
            color: COLORS.diskWrite,
            value: latest ? formatMBps(latest.disk_write_bytes_sec) : '—',
            dataKey: 'disk_write_mbs',
            primary: true,
          },
        ]}
      />,
    ],
  ];

  return (
    <div
      className="
        flex flex-col md:grid md:grid-rows-3
        flex-1 min-h-0
        bg-bg-card border border-border rounded-lg overflow-hidden
      "
    >
      {rows.map((row, rIdx) => (
        <div
          key={rIdx}
          className={`
            flex flex-col md:grid md:grid-cols-2
            md:flex-initial flex-initial
            min-h-0
            ${rIdx > 0 ? 'border-t border-border' : ''}
          `}
        >
          {row.map((cell, cIdx) => (
            <div
              key={cIdx}
              className={`
                relative h-[180px] md:h-auto md:min-h-0
                ${cIdx > 0 ? 'border-t md:border-t-0 md:border-l border-border' : ''}
              `}
            >
              {cell}
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}
