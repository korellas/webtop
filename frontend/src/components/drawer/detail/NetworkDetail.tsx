import { useEffect, useState } from 'react';
import {
  BarChart, Bar, XAxis, YAxis, ResponsiveContainer, Tooltip, CartesianGrid,
} from 'recharts';
import { COLORS } from '../../../lib/colors';
import { formatBytes, formatMBps } from '../../../lib/format';
import { fetchNetworkHistory } from '../../../lib/api';
import { useNetworkStore } from '../../../store/network-store';
import { useMetricsStore } from '../../../store/metrics-store';
import { useTimescaleStore } from '../../../store/timescale-store';
import DrawerHeader from '../DrawerHeader';
import type { NetInterfaceInfo, NetworkHistory } from '../../../lib/types';
import type { DetailProps } from '../DrawerContent';

type Group = 'hour' | 'day' | 'week' | 'month';

const HISTORY_TABS: { id: Group; label: string }[] = [
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

/** Trim trailing .0 so "2.5 Gbps" stays 2.5 and "1.0 Gbps" becomes "1 Gbps". */
function trimTrailingZero(n: number, decimals: number): string {
  const fixed = n.toFixed(decimals);
  return fixed.replace(/\.?0+$/, '');
}

function formatSpeed(bps: number | null): string {
  if (!bps || bps <= 0) return '—';
  if (bps >= 1_000_000_000) {
    // 1 decimal when needed (2.5 Gbps, 5.0 Gbps → "5 Gbps"). Never round
    // fractional speeds to a whole number — 2.5 Gbps must not show as 3 Gbps.
    return `${trimTrailingZero(bps / 1_000_000_000, 1)} Gbps`;
  }
  if (bps >= 1_000_000) {
    // Mbps is almost always a round number (100, 324, 1000) so no decimals.
    return `${Math.round(bps / 1_000_000)} Mbps`;
  }
  return `${Math.round(bps / 1000)} Kbps`;
}

/** RSSI → qualitative quality so users don't need to decode dBm. */
function rssiQuality(dbm: number | null): { label: string; color: string } {
  if (dbm === null) return { label: '—', color: 'var(--color-text-muted)' };
  if (dbm >= -55) return { label: 'Excellent', color: 'var(--color-gpu)' };
  if (dbm >= -65) return { label: 'Good',      color: 'var(--color-gpu)' };
  if (dbm >= -75) return { label: 'Fair',      color: 'var(--color-warning)' };
  return { label: 'Weak', color: 'var(--color-danger)' };
}

const KIND_LABELS: Record<NetInterfaceInfo['kind'], string> = {
  wifi: 'Wi-Fi',
  ethernet: 'Ethernet',
  bridge: 'Bridge',
  vpn: 'VPN',
  loopback: 'Loopback',
  p2p: 'Peer-to-peer',
  other: 'Other',
};

export default function NetworkDetail({ onClose }: DetailProps) {
  // Data is kept fresh by the app-level `useNetworkPoll` hook; we just read it.
  const interfaces = useNetworkStore((s) => s.interfaces);
  const loaded = useNetworkStore((s) => s.loaded);
  // Same figure the Network chart's Σ▲/Σ▼ pills show — kept fresh by
  // `loadNetworkTotals` in `use-history`. Repeated here full-width because
  // the chart's title-row pills truncate on narrow (mobile) viewports.
  const networkTotals = useMetricsStore((s) => s.networkTotals);
  const timescale = useTimescaleStore((s) => s.timescale);

  // Hour/Day/Week/Month breakdown, mirroring the Energy drawer's tabs —
  // independent of the rolling `networkTotals` above, which only covers
  // whatever timescale the main dashboard happens to have selected.
  const [group, setGroup] = useState<Group>('hour');
  const [history, setHistory] = useState<NetworkHistory | null>(null);
  const [historyLoading, setHistoryLoading] = useState(true);

  useEffect(() => {
    const ctrl = new AbortController();
    let cancelled = false;
    setHistoryLoading(true);

    fetchNetworkHistory(group, ctrl.signal)
      .then((d) => {
        if (!cancelled) {
          setHistory(d);
          setHistoryLoading(false);
        }
      })
      .catch(() => {
        if (!cancelled) setHistoryLoading(false);
      });

    return () => {
      cancelled = true;
      ctrl.abort();
    };
  }, [group]);

  const chartData = (history?.buckets ?? []).map((b) => ({
    ts: b.bucket_start,
    up: b.up_bytes,
    down: b.down_bytes,
  }));

  return (
    <div className="pb-2">
      <DrawerHeader
        color={COLORS.network}
        label="Net"
        value={interfaces ? `${interfaces.length} active` : undefined}
        labelId="drawer-title"
        onClose={onClose}
      />

      <div className="pt-4">
        {networkTotals && (
          <div className="flex items-center justify-around p-3 mb-3 bg-bg-hover/30 rounded-lg border border-border text-[11px]">
            <TotalStat
              label={`▲ Sent (${timescale})`}
              value={formatBytes(networkTotals.up_bytes)}
              color={COLORS.networkLight}
            />
            <span className="w-px h-6 bg-border" />
            <TotalStat
              label={`▼ Received (${timescale})`}
              value={formatBytes(networkTotals.down_bytes)}
              color={COLORS.network}
            />
          </div>
        )}

        {/* Hour/Day/Week/Month breakdown */}
        <div className="space-y-4 mb-4">
          <div className="flex items-center justify-between gap-2">
            <div className="flex gap-1 p-1 bg-bg-hover/40 rounded-md w-fit">
              {HISTORY_TABS.map((t) => (
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
            {/* Two bars per bucket need a key — the tooltip labels them too,
                but that only helps on hover. */}
            <div className="flex items-center gap-3 text-[10px] text-text-secondary">
              <span className="flex items-center gap-1">
                <span className="w-2 h-2 rounded-sm" style={{ backgroundColor: COLORS.networkLight }} />
                Sent
              </span>
              <span className="flex items-center gap-1">
                <span className="w-2 h-2 rounded-sm" style={{ backgroundColor: COLORS.network }} />
                Received
              </span>
            </div>
          </div>

          <div className="h-48 w-full">
            {historyLoading && (
              <div className="h-full flex items-center justify-center text-text-muted text-sm">
                Loading…
              </div>
            )}
            {!historyLoading && chartData.length === 0 && (
              <div className="h-full flex items-center justify-center text-text-muted text-sm text-center px-4">
                No network history yet — come back in an hour.
              </div>
            )}
            {!historyLoading && chartData.length > 0 && (
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
                    tickFormatter={(v) => formatBytes(v)}
                    tick={{ fontSize: 9, fill: 'var(--color-chart-axis)' }}
                    stroke="transparent"
                    width={40}
                    axisLine={false}
                    tickLine={false}
                  />
                  <Tooltip
                    cursor={{ fill: 'var(--color-bg-hover)', opacity: 0.5 }}
                    content={({ active, payload }) => {
                      if (!active || !payload?.length) return null;
                      const ts = payload[0].payload.ts as number;
                      const up = payload[0].payload.up as number;
                      const down = payload[0].payload.down as number;
                      return (
                        <div
                          className="bg-bg-card border border-border-strong rounded-lg px-2.5 py-2 shadow-lg text-[11px]"
                          style={{ pointerEvents: 'none' }}
                        >
                          <div className="text-text-muted text-[10px] mb-1">
                            {formatBucketTooltip(ts, group)}
                          </div>
                          <div className="flex items-center gap-1.5" style={{ color: COLORS.networkLight }}>
                            <span>▲</span>
                            <span className="font-semibold tabular-nums">{formatBytes(up)}</span>
                          </div>
                          <div className="flex items-center gap-1.5" style={{ color: COLORS.network }}>
                            <span>▼</span>
                            <span className="font-semibold tabular-nums">{formatBytes(down)}</span>
                          </div>
                        </div>
                      );
                    }}
                  />
                  <Bar dataKey="up" fill={COLORS.networkLight} radius={[3, 3, 0, 0]} isAnimationActive={false} />
                  <Bar dataKey="down" fill={COLORS.network} radius={[3, 3, 0, 0]} isAnimationActive={false} />
                </BarChart>
              </ResponsiveContainer>
            )}
          </div>

          {!historyLoading && history && (
            <div className="flex items-center justify-around p-3 bg-bg-hover/30 rounded-lg border border-border text-[11px]">
              <TotalStat label="▲ Total" value={formatBytes(history.total_up_bytes)} color={COLORS.networkLight} />
              <span className="w-px h-6 bg-border" />
              <TotalStat label="▼ Total" value={formatBytes(history.total_down_bytes)} color={COLORS.network} />
            </div>
          )}
        </div>

        {!loaded && interfaces === null && (
          <div className="py-6 text-center text-text-muted text-sm">Loading…</div>
        )}

        {loaded && (interfaces?.length ?? 0) === 0 && (
          <div className="py-6 text-center text-text-muted text-sm">
            No active interfaces with an IP address.
          </div>
        )}

        <div className="space-y-2">
          {interfaces?.map((iface) => (
            <InterfaceRow key={iface.name} iface={iface} />
          ))}
        </div>
      </div>
    </div>
  );
}

function InterfaceRow({ iface }: { iface: NetInterfaceInfo }) {
  const ipv6Local = iface.ipv6.filter((ip) => ip.toLowerCase().startsWith('fe80'));
  const ipv6Global = iface.ipv6.filter((ip) => !ip.toLowerCase().startsWith('fe80'));
  const w = iface.wireless;
  const rssi = rssiQuality(w?.rssi_dbm ?? null);
  const ssidHidden = iface.kind === 'wifi' && !w?.ssid && w !== null && w !== undefined;

  return (
    <div className="border border-border rounded-lg p-3 bg-bg-hover/30">
      <div className="flex items-center justify-between mb-2">
        <div className="flex items-center gap-2 min-w-0">
          <span
            className="w-2.5 h-2.5 rounded-full shrink-0"
            style={{
              backgroundColor: iface.is_up ? 'var(--color-gpu)' : 'var(--color-text-muted)',
            }}
          />
          <span className="text-[15px] font-bold font-mono">{iface.name}</span>
          <span className="text-[11px] text-text-secondary px-1.5 py-0.5 rounded bg-bg-primary/60">
            {KIND_LABELS[iface.kind]}
          </span>
        </div>
        {iface.link_speed_bps && (
          <span className="text-[12px] font-semibold text-text-primary tabular-nums">
            {formatSpeed(iface.link_speed_bps)}
          </span>
        )}
      </div>

      {/* Wi-Fi SSID badge (only for wifi) */}
      {iface.kind === 'wifi' && (w?.ssid || ssidHidden) && (
        <div className="flex items-center gap-2 mb-2 pb-2 border-b border-border">
          <svg
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="var(--color-gpu)"
            strokeWidth="2.2"
            strokeLinecap="round"
            strokeLinejoin="round"
            className="shrink-0"
          >
            <path d="M5 12.55a11 11 0 0 1 14.08 0" />
            <path d="M1.42 9a16 16 0 0 1 21.16 0" />
            <path d="M8.53 16.11a6 6 0 0 1 6.95 0" />
            <path d="M12 20h.01" />
          </svg>
          {w?.ssid ? (
            <span className="text-[14px] font-semibold truncate">{w.ssid}</span>
          ) : (
            <span
              className="text-[12px] italic text-text-muted truncate"
              title="macOS 14+ hides Wi-Fi SSIDs from apps without Location Services permission."
            >
              SSID hidden by macOS privacy
            </span>
          )}
          {w?.security && w.security.toLowerCase() !== 'none' && (
            <span className="text-[9px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-bg-hover text-text-secondary">
              {w.security}
            </span>
          )}
        </div>
      )}

      {/* Wi-Fi stats grid */}
      {iface.kind === 'wifi' && w && (
        <div className="grid grid-cols-2 gap-x-4 gap-y-1.5 text-[12px] mb-2.5">
          {w.rssi_dbm !== null && (
            <Stat
              k="Signal"
              v={
                <>
                  <span className="tabular-nums">{w.rssi_dbm} dBm</span>
                  <span className="ml-1 font-semibold" style={{ color: rssi.color }}>
                    ({rssi.label})
                  </span>
                </>
              }
            />
          )}
          {w.tx_rate_mbps !== null && (
            <Stat k="TX rate" v={`${w.tx_rate_mbps} Mbps`} />
          )}
          {w.channel !== null && (
            <Stat
              k="Channel"
              v={`${w.channel}${w.band ? ` (${w.band})` : ''}`}
            />
          )}
          {w.bssid && <Stat k="BSSID" v={w.bssid} mono />}
        </div>
      )}

      {/* Addresses */}
      <div className="space-y-1 text-[12px] font-mono text-text-secondary mb-2">
        {iface.ipv4.length > 0 && <InfoRow k="IPv4" v={iface.ipv4.join(', ')} />}
        {ipv6Global.length > 0 && <InfoRow k="IPv6" v={ipv6Global.join(', ')} />}
        {ipv6Local.length > 0 && !ipv6Global.length && (
          <InfoRow k="IPv6" v={`${ipv6Local[0]} (link-local)`} />
        )}
        {iface.mac && <InfoRow k="MAC" v={iface.mac} />}
        {iface.mtu && <InfoRow k="MTU" v={String(iface.mtu)} />}
      </div>

      {/* Live traffic */}
      <div className="flex items-center gap-4 text-[12px] font-semibold tabular-nums">
        <span className="flex items-center gap-1.5">
          <span style={{ color: COLORS.networkLight }}>▲</span>
          {formatMBps(iface.tx_bytes_sec)}
        </span>
        <span className="flex items-center gap-1.5">
          <span style={{ color: COLORS.network }}>▼</span>
          {formatMBps(iface.rx_bytes_sec)}
        </span>
      </div>
    </div>
  );
}

function TotalStat({ label, value, color }: { label: string; value: string; color: string }) {
  return (
    <div className="flex flex-col items-center gap-0.5">
      <span className="text-[9px] uppercase tracking-wider text-text-muted">{label}</span>
      <span className="font-semibold tabular-nums" style={{ color }}>{value}</span>
    </div>
  );
}

function Stat({ k, v, mono }: { k: string; v: React.ReactNode; mono?: boolean }) {
  return (
    <div className="flex items-baseline gap-2 min-w-0">
      <span className="text-text-muted shrink-0 w-14 text-[10px] uppercase tracking-wider">{k}</span>
      <span className={`truncate ${mono ? 'font-mono text-[11px]' : ''}`}>{v}</span>
    </div>
  );
}

function InfoRow({ k, v }: { k: string; v: string }) {
  return (
    <div className="flex gap-2">
      <span className="w-10 text-text-muted shrink-0 text-[10px] uppercase tracking-wider">{k}</span>
      <span className="break-all text-text-primary">{v}</span>
    </div>
  );
}
