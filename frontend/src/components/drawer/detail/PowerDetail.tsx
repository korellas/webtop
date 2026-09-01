import { useMetricsStore } from '../../../store/metrics-store';
import { COLORS } from '../../../lib/colors';
import { formatWatts } from '../../../lib/format';
import DrawerHeader from '../DrawerHeader';
import type { BatteryInfo } from '../../../lib/types';
import type { DetailProps } from '../DrawerContent';

function formatTime(sec: number | null | undefined): string {
  if (sec === null || sec === undefined || sec <= 0) return '—';
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return h > 0 ? `${h}h ${m}m` : `${m}m`;
}

export default function PowerDetail({ onClose }: DetailProps) {
  const latest = useMetricsStore((s) => s.snapshots[s.snapshots.length - 1]);
  const battery = latest?.battery ?? null;

  const totalW = latest?.power_total_w ?? 0;
  const cpuW = latest?.power_cpu_w ?? 0;
  const gpuW = latest?.power_gpu_w ?? 0;
  const otherW = latest?.power_other_w ?? 0;

  // Normalize bars to the max visible subsystem so the smallest is still visible.
  const maxW = Math.max(totalW, cpuW, gpuW, otherW, 1);

  return (
    <div className="pb-2">
      <DrawerHeader
        color={COLORS.power}
        label="Power"
        value={latest ? formatWatts(totalW) : undefined}
        labelId="drawer-title"
        onClose={onClose}
      />

      <div className="pt-4 space-y-5">
        {battery && <BatteryBlock battery={battery} />}

        <section>
          <h3 className="text-[11px] uppercase tracking-wider text-text-secondary font-semibold mb-2">
            Power Breakdown
          </h3>
          <div className="space-y-2">
            <PowerRow label="Total" value={totalW} maxW={maxW} color={COLORS.power} strong />
            <PowerRow label="CPU" value={cpuW} maxW={maxW} color={COLORS.powerCpu} />
            <PowerRow label="GPU" value={gpuW} maxW={maxW} color={COLORS.powerGpu} />
            <PowerRow label="Other" value={otherW} maxW={maxW} color={COLORS.powerOther} />
          </div>
          <div className="text-[10px] text-text-muted mt-3 leading-relaxed">
            “Other” includes DRAM, display, Wi-Fi, SoC fabric, and the ANE.
          </div>
        </section>

        {!battery && (
          <div className="text-[11px] text-text-muted text-center py-2 bg-bg-hover/40 rounded-md border border-border">
            No battery detected — desktop Mac.
          </div>
        )}
      </div>
    </div>
  );
}

function BatteryBlock({ battery }: { battery: BatteryInfo }) {
  const pct = Math.max(0, Math.min(100, battery.percent));
  const healthColor =
    battery.health_percent === null
      ? 'var(--color-text-secondary)'
      : battery.health_percent >= 90
      ? 'var(--color-gpu)'
      : battery.health_percent >= 75
      ? 'var(--color-warning)'
      : 'var(--color-danger)';

  const stateLabel = battery.is_charging
    ? 'Charging'
    : battery.is_plugged_in
    ? 'Plugged in'
    : 'Discharging';

  return (
    <section>
      <h3 className="text-[11px] uppercase tracking-wider text-text-secondary font-semibold mb-2">
        Battery
      </h3>
      <div className="border border-border rounded-lg p-3 bg-bg-hover/30">
        {/* Percentage bar */}
        <div className="flex items-center gap-3 mb-3">
          <div className="flex-1 h-3 rounded-md bg-bg-primary overflow-hidden border border-border">
            <div
              className="h-full rounded-sm transition-[width] duration-500"
              style={{
                width: `${pct}%`,
                backgroundColor: pct < 20
                  ? 'var(--color-danger)'
                  : pct < 40
                  ? 'var(--color-warning)'
                  : 'var(--color-gpu)',
              }}
              role="progressbar"
              aria-valuenow={Math.round(pct)}
              aria-valuemin={0}
              aria-valuemax={100}
            />
          </div>
          <span className="text-[16px] font-bold tabular-nums">{pct.toFixed(0)}%</span>
        </div>

        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px]">
          <span className="font-semibold">{stateLabel}</span>
          {battery.charge_rate_w !== null && Math.abs(battery.charge_rate_w) > 0.1 && (
            <span className="text-text-secondary tabular-nums">
              {battery.charge_rate_w > 0 ? '+' : ''}
              {battery.charge_rate_w.toFixed(1)} W
            </span>
          )}
          {battery.time_remaining_sec !== null && (
            <span className="text-text-secondary">
              {formatTime(battery.time_remaining_sec)} remaining
            </span>
          )}
        </div>

        {(battery.cycle_count !== null || battery.health_percent !== null) && (
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[10px] text-text-muted mt-2 pt-2 border-t border-border">
            {battery.cycle_count !== null && (
              <span>
                Cycle <span className="tabular-nums font-semibold">{battery.cycle_count}</span>
              </span>
            )}
            {battery.health_percent !== null && (
              <span>
                Health{' '}
                <span
                  className="tabular-nums font-semibold"
                  style={{ color: healthColor }}
                >
                  {battery.health_percent.toFixed(0)}%
                </span>
              </span>
            )}
          </div>
        )}
      </div>
    </section>
  );
}

function PowerRow({
  label,
  value,
  maxW,
  color,
  strong,
}: {
  label: string;
  value: number;
  maxW: number;
  color: string;
  strong?: boolean;
}) {
  const pct = maxW > 0 ? (value / maxW) * 100 : 0;
  return (
    <div className="flex items-center gap-2 text-[11px]">
      <span className={`w-14 ${strong ? 'font-semibold' : 'text-text-secondary'}`}>{label}</span>
      <div className="flex-1 h-2.5 rounded bg-bg-hover overflow-hidden">
        <div
          className="h-full rounded transition-[width] duration-300"
          style={{
            width: `${Math.max(1, Math.min(100, pct))}%`,
            backgroundColor: color,
          }}
        />
      </div>
      <span className={`w-14 text-right tabular-nums shrink-0 ${strong ? 'font-bold' : 'font-semibold'}`}>
        {formatWatts(value)}
      </span>
    </div>
  );
}
