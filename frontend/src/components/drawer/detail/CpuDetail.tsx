import { useMetricsStore } from '../../../store/metrics-store';
import { useSystemStore } from '../../../store/system-store';
import { COLORS } from '../../../lib/colors';
import DrawerHeader from '../DrawerHeader';
import type { DetailProps } from '../DrawerContent';

export default function CpuDetail({ onClose }: DetailProps) {
  const latest = useMetricsStore((s) => s.snapshots[s.snapshots.length - 1]);
  const info = useSystemStore((s) => s.info);

  const cores = latest?.cpu_cores ?? [];
  const kinds = info?.core_kinds ?? [];

  const paired = cores.map((usage, i) => ({
    usage,
    kind: (kinds[i] ?? 'P') as 'P' | 'E',
    index: i,
  }));

  const pCores = paired.filter((c) => c.kind === 'P');
  const eCores = paired.filter((c) => c.kind === 'E');

  return (
    <div className="pb-2">
      <DrawerHeader
        color={COLORS.compute}
        label="CPU"
        value={latest ? `${Math.round(latest.cpu_total)}%` : undefined}
        labelId="drawer-title"
        onClose={onClose}
      />

      {!latest && (
        <div className="py-8 text-center text-text-muted text-sm">Waiting for data…</div>
      )}

      {latest && paired.length === 0 && (
        <div className="py-4 text-center text-text-muted text-sm">
          Per-core data unavailable.
        </div>
      )}

      {latest && paired.length > 0 && (
        <div className="pt-4 space-y-5">
          {pCores.length > 0 && (
            <CoreGroup
              title="Performance cores"
              count={pCores.length}
              cores={pCores}
              color={COLORS.compute}
              prefix="P"
            />
          )}
          {eCores.length > 0 && (
            <CoreGroup
              title="Efficiency cores"
              count={eCores.length}
              cores={eCores}
              color={COLORS.computeLight}
              prefix="E"
            />
          )}
        </div>
      )}
    </div>
  );
}

function CoreGroup({
  title,
  count,
  cores,
  color,
  prefix,
}: {
  title: string;
  count: number;
  cores: { usage: number; index: number }[];
  color: string;
  prefix: string;
}) {
  return (
    <section>
      <div className="flex items-baseline justify-between mb-2">
        <h3 className="text-[11px] uppercase tracking-wider text-text-secondary font-semibold">
          {title}
        </h3>
        <span className="text-[11px] tabular-nums text-text-muted">{count}</span>
      </div>
      {/* 2-column grid — no vertical scrolling even with 20 P-cores. */}
      <div className="grid grid-cols-2 gap-x-4 gap-y-1.5">
        {cores.map((c, i) => (
          <CoreBar key={c.index} label={`${prefix}${i}`} usage={c.usage} color={color} />
        ))}
      </div>
    </section>
  );
}

function CoreBar({ label, usage, color }: { label: string; usage: number; color: string }) {
  const clamped = Math.max(0, Math.min(100, usage));
  return (
    <div className="flex items-center gap-2 text-[11px]">
      <span className="w-6 text-text-muted font-mono tabular-nums shrink-0">{label}</span>
      <div className="flex-1 h-2 rounded bg-bg-hover overflow-hidden">
        <div
          className="h-full rounded transition-[width] duration-300"
          style={{ width: `${clamped}%`, backgroundColor: color }}
          role="progressbar"
          aria-valuenow={Math.round(clamped)}
          aria-valuemin={0}
          aria-valuemax={100}
        />
      </div>
      <span className="w-8 text-right tabular-nums font-semibold shrink-0">
        {usage < 10 ? usage.toFixed(1) : Math.round(usage)}
      </span>
    </div>
  );
}
