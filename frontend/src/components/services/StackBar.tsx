import { formatBytes } from '../../lib/format';
import type { ServiceStatus } from '../../lib/types';

interface Props {
  services: ServiceStatus[];
  /** Physical RAM. The bar means nothing without a real denominator. */
  memTotal: number;
}

/**
 * The managed stack's footprint as a fraction of physical memory.
 *
 * This is the one question the per-service rows cannot answer, because they
 * share an axis scaled to the largest single service rather than to the
 * machine. On a box where the declared budgets sum to more than the RAM
 * installed, "how much is left" is what decides whether the next model loads
 * at all.
 *
 * One hue, with hairline separators between services. Colouring the segments
 * per group made this bar a second colour language competing with status for
 * attention; the segment boundaries alone still show the composition, and the
 * one thing that has to be noticed on this screen stays the only coloured
 * thing.
 */
export default function StackBar({ services, memTotal }: Props) {
  const used = services.reduce((sum, s) => sum + s.mem_bytes, 0);
  if (memTotal <= 0) return null;

  const pct = (bytes: number) => (bytes / memTotal) * 100;

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-baseline gap-2">
        <span className="text-[9px] uppercase tracking-wider text-text-secondary">Stack</span>
        <span className="text-[15px] font-semibold tabular-nums">{formatBytes(used)}</span>
        <span className="text-[11px] text-text-muted tabular-nums">
          of {formatBytes(memTotal)} · {Math.round(pct(used))}%
        </span>
        <span className="ml-auto text-[11px] text-text-secondary tabular-nums">
          {formatBytes(memTotal - used)} free
        </span>
      </div>

      <div className="h-2.5 rounded bg-bg-hover flex overflow-hidden">
        {services
          .filter((s) => s.mem_bytes > 0)
          .map((s) => (
            <span
              key={s.name}
              className="h-full transition-[width] duration-500 border-r border-bg-primary/50 last:border-r-0"
              style={{ width: `${pct(s.mem_bytes)}%`, background: 'var(--color-memory)' }}
              title={`${s.name} — ${formatBytes(s.mem_bytes)}`}
            />
          ))}
      </div>
    </div>
  );
}
