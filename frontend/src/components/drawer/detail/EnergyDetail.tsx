import { useEffect, useState } from 'react';
import {
  BarChart, Bar, XAxis, YAxis, ResponsiveContainer, Tooltip, CartesianGrid,
} from 'recharts';
import { useMetricsStore } from '../../../store/metrics-store';
import { COLORS } from '../../../lib/colors';
import { formatWh } from '../../../lib/format';
import { fetchEnergyHistory } from '../../../lib/api';
import DrawerHeader from '../DrawerHeader';
import type { EnergyHistory } from '../../../lib/types';
import type { DetailProps } from '../DrawerContent';

type Group = 'hour' | 'day' | 'week' | 'month';

const TABS: { id: Group; label: string }[] = [
  { id: 'hour',  label: 'Hour' },
  { id: 'day',   label: 'Day' },
  { id: 'week',  label: 'Week' },
  { id: 'month', label: 'Month' },
];

function formatBucketX(ts: number, group: Group): string {
  const d = new Date(ts);
  switch (group) {
    case 'hour':  return d.getHours().toString().padStart(2, '0');
    case 'day':   return `${d.getMonth() + 1}/${d.getDate()}`;
    case 'week':  {
      // ISO-ish week number
      const onejan = new Date(d.getFullYear(), 0, 1);
      const week = Math.ceil(((d.getTime() - onejan.getTime()) / 86400000 + onejan.getDay() + 1) / 7);
      return `W${week}`;
    }
    case 'month': return d.toLocaleString('default', { month: 'short' });
  }
}

function formatBucketTooltip(ts: number, group: Group): string {
  const d = new Date(ts);
  switch (group) {
    case 'hour':
      return d.toLocaleString([], { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' });
    case 'day':
      return d.toLocaleDateString([], { year: 'numeric', month: 'short', day: 'numeric' });
    case 'week':
      return `Week of ${d.toLocaleDateString([], { month: 'short', day: 'numeric' })}`;
    case 'month':
      return d.toLocaleDateString([], { year: 'numeric', month: 'long' });
  }
}

export default function EnergyDetail({ onClose }: DetailProps) {
  const latest = useMetricsStore((s) => s.snapshots[s.snapshots.length - 1]);
  const [group, setGroup] = useState<Group>('hour');
  const [data, setData] = useState<EnergyHistory | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    const ctrl = new AbortController();
    let cancelled = false;
    setLoading(true);

    fetchEnergyHistory(group, ctrl.signal)
      .then((d) => {
        if (!cancelled) {
          setData(d);
          setLoading(false);
        }
      })
      .catch(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
      ctrl.abort();
    };
  }, [group]);

  const sessionWh = latest?.energy_session_wh ?? 0;

  const chartData = (data?.buckets ?? []).map((b) => ({
    ts: b.bucket_start,
    wh: Number(b.wh.toFixed(2)),
  }));

  return (
    <div className="pb-2">
      <DrawerHeader
        color={COLORS.power}
        label="Energy"
        value={sessionWh > 0 ? `${formatWh(sessionWh)} this session` : undefined}
        labelId="drawer-title"
        onClose={onClose}
      />

      <div className="pt-4 space-y-4">
        {/* Tab selector */}
        <div className="flex gap-1 p-1 bg-bg-hover/40 rounded-md w-fit">
          {TABS.map((t) => (
            <button
              key={t.id}
              type="button"
              onClick={() => setGroup(t.id)}
              className={`
                px-3 py-1 rounded text-[11px] font-semibold
                transition-colors
                ${
                  group === t.id
                    ? 'bg-bg-card text-text-primary shadow-sm'
                    : 'text-text-secondary hover:text-text-primary'
                }
              `}
            >
              {t.label}
            </button>
          ))}
        </div>

        {/* Chart */}
        <div className="h-48 w-full">
          {loading && (
            <div className="h-full flex items-center justify-center text-text-muted text-sm">
              Loading…
            </div>
          )}
          {!loading && chartData.length === 0 && (
            <div className="h-full flex items-center justify-center text-text-muted text-sm text-center px-4">
              No energy data yet — come back in an hour.
            </div>
          )}
          {!loading && chartData.length > 0 && (
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={chartData} margin={{ top: 4, right: 4, left: 4, bottom: 2 }}>
                <CartesianGrid
                  stroke="var(--color-chart-grid)"
                  strokeDasharray="2 4"
                  vertical={false}
                />
                <XAxis
                  dataKey="ts"
                  tickFormatter={(v) => formatBucketX(v, group)}
                  tick={{ fontSize: 9, fill: 'var(--color-chart-axis)' }}
                  stroke="var(--color-chart-grid)"
                  interval="preserveStartEnd"
                />
                <YAxis
                  tickFormatter={(v) => (v < 1000 ? `${Math.round(v)}` : `${(v / 1000).toFixed(1)}k`)}
                  tick={{ fontSize: 9, fill: 'var(--color-chart-axis)' }}
                  stroke="transparent"
                  width={34}
                  axisLine={false}
                  tickLine={false}
                />
                <Tooltip
                  cursor={{ fill: 'var(--color-bg-hover)', opacity: 0.5 }}
                  content={({ active, payload }) => {
                    if (!active || !payload?.length) return null;
                    const p = payload[0];
                    const ts = p.payload.ts as number;
                    const wh = p.payload.wh as number;
                    return (
                      <div
                        className="bg-bg-card border border-border-strong rounded-lg px-2.5 py-2 shadow-lg text-[11px]"
                        style={{ pointerEvents: 'none' }}
                      >
                        <div className="text-text-muted text-[10px]">
                          {formatBucketTooltip(ts, group)}
                        </div>
                        <div className="font-semibold tabular-nums">{formatWh(wh)}</div>
                      </div>
                    );
                  }}
                />
                <Bar
                  dataKey="wh"
                  fill={COLORS.power}
                  radius={[3, 3, 0, 0]}
                  isAnimationActive={false}
                />
              </BarChart>
            </ResponsiveContainer>
          )}
        </div>

        {/* Summary row */}
        {!loading && data && (
          <div className="flex items-center justify-around p-3 bg-bg-hover/30 rounded-lg border border-border text-[11px]">
            <SummaryStat label="Total" value={formatWh(data.total_wh)} />
            <span className="w-px h-6 bg-border" />
            <SummaryStat label="Avg" value={formatWh(data.avg_wh)} />
            <span className="w-px h-6 bg-border" />
            <SummaryStat label="Peak" value={formatWh(data.peak_wh)} />
          </div>
        )}
      </div>
    </div>
  );
}

function SummaryStat({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col items-center gap-0.5">
      <span className="text-[9px] uppercase tracking-wider text-text-muted">{label}</span>
      <span className="font-semibold tabular-nums">{value}</span>
    </div>
  );
}
