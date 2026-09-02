import { useMetricsStore } from '../../../store/metrics-store';
import { useSystemStore } from '../../../store/system-store';
import { COLORS } from '../../../lib/colors';
import { formatGB } from '../../../lib/format';
import DrawerHeader from '../DrawerHeader';
import type { DetailProps } from '../DrawerContent';

interface Segment {
  label: string;
  bytes: number;
  color: string;
}

export default function RamDetail({ onClose }: DetailProps) {
  const latest = useMetricsStore((s) => s.snapshots[s.snapshots.length - 1]);
  const info = useSystemStore((s) => s.info);

  const memTotal = info?.mem_total ?? 0;
  const bd = latest?.mem_breakdown ?? null;
  const memUsed = latest?.mem_used ?? 0;
  const memPct = memTotal > 0 ? (memUsed / memTotal) * 100 : 0;
  const swapUsed = latest?.mem_swap_used ?? 0;

  const segments: Segment[] = bd
    ? [
        { label: 'Wired',      bytes: bd.wired,      color: 'var(--color-cpu)' },
        { label: 'Active',     bytes: bd.active,     color: 'var(--color-memory)' },
        // Dim/desaturated on purpose — distinct from `--color-memory-swap`
        // (the disk-swap line on the Memory chart, an unrelated meaning)
        // and dark enough not to read as a second "active" segment.
        { label: 'Inactive',   bytes: bd.inactive,   color: 'var(--color-memory-inactive)' },
        { label: 'Compressed', bytes: bd.compressed, color: 'var(--color-power)' },
        { label: 'Free',       bytes: bd.free,       color: 'var(--color-bg-hover)' },
      ]
    : [];

  const segTotal = segments.reduce((a, s) => a + s.bytes, 0);
  // Pressure: compressed / (total - free)
  const nonFree = segTotal - (bd?.free ?? 0);
  const pressureRatio = nonFree > 0 ? (bd?.compressed ?? 0) / nonFree : 0;
  const pressure = pressureRatio < 0.15
    ? { label: 'Normal', color: 'var(--color-gpu)' }
    : pressureRatio < 0.30
    ? { label: 'Elevated', color: 'var(--color-warning)' }
    : { label: 'Critical', color: 'var(--color-danger)' };

  return (
    <div className="pb-2">
      <DrawerHeader
        color={COLORS.memory}
        label="RAM"
        value={
          latest
            ? `${memPct.toFixed(0)}% (${formatGB(memUsed, 1)} / ${formatGB(memTotal, 0)})`
            : undefined
        }
        labelId="drawer-title"
        onClose={onClose}
      />

      {!latest && (
        <div className="py-8 text-center text-text-muted text-sm">Waiting for data…</div>
      )}

      {latest && bd && (
        <div className="pt-4 space-y-5">
          {/* Stacked bar */}
          <div className="h-5 rounded-md overflow-hidden flex border border-border">
            {segments.map((seg) => {
              const pct = segTotal > 0 ? (seg.bytes / segTotal) * 100 : 0;
              if (pct < 0.1) return null;
              return (
                <div
                  key={seg.label}
                  style={{ width: `${pct}%`, backgroundColor: seg.color }}
                  title={`${seg.label} ${formatGB(seg.bytes, 1)}`}
                />
              );
            })}
          </div>

          {/* Legend */}
          <div className="grid grid-cols-2 sm:grid-cols-3 gap-x-4 gap-y-2 text-[11px]">
            {segments.map((seg) => (
              <div key={seg.label} className="flex items-center gap-2">
                <span
                  className="w-2.5 h-2.5 rounded-sm shrink-0 border border-border"
                  style={{ backgroundColor: seg.color }}
                />
                <span className="text-text-secondary">{seg.label}</span>
                <span className="ml-auto tabular-nums font-semibold">{formatGB(seg.bytes, 1)}</span>
              </div>
            ))}
          </div>

          {/* Pressure */}
          <div className="flex items-center justify-between p-2.5 bg-bg-hover/50 rounded-lg border border-border">
            <div>
              <div className="text-[11px] uppercase tracking-wider text-text-secondary">
                Memory Pressure
              </div>
              <div className="text-[13px] font-semibold mt-0.5" style={{ color: pressure.color }}>
                {pressure.label}
              </div>
            </div>
            <div className="text-right">
              <div className="text-[11px] uppercase tracking-wider text-text-secondary">Ratio</div>
              <div className="text-[13px] font-semibold tabular-nums mt-0.5">
                {(pressureRatio * 100).toFixed(1)}%
              </div>
            </div>
          </div>

          {/* Swap */}
          {swapUsed > 0 && (
            <div className="flex items-center justify-between p-2.5 bg-bg-hover/50 rounded-lg border border-border">
              <div>
                <div className="text-[11px] uppercase tracking-wider text-text-secondary">Swap</div>
                <div className="text-[13px] font-semibold mt-0.5 tabular-nums">
                  {formatGB(swapUsed, 2)}
                </div>
              </div>
            </div>
          )}
        </div>
      )}

      {latest && !bd && (
        <div className="py-4 text-center text-text-muted text-sm">
          Memory breakdown unavailable.
        </div>
      )}
    </div>
  );
}
