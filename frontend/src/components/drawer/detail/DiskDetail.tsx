import { useEffect, useState } from 'react';
import { COLORS } from '../../../lib/colors';
import { formatGB } from '../../../lib/format';
import { fetchDisks } from '../../../lib/api';
import DrawerHeader from '../DrawerHeader';
import FolderTree from './FolderTree';
import type { DiskInfo } from '../../../lib/types';
import type { DetailProps } from '../DrawerContent';

export default function DiskDetail({ onClose }: DetailProps) {
  const [disks, setDisks] = useState<DiskInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const ctrl = new AbortController();
    let cancelled = false;
    fetchDisks(ctrl.signal)
      .then((d) => {
        if (!cancelled) setDisks(d);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      });
    return () => {
      cancelled = true;
      ctrl.abort();
    };
  }, []);

  const count = disks?.length ?? 0;

  return (
    <div className="pb-2">
      <DrawerHeader
        color={COLORS.storage}
        label="Disk"
        value={count > 0 ? `${count} volumes` : undefined}
        labelId="drawer-title"
        onClose={onClose}
      />

      <div className="pt-4">
        {disks === null && !error && (
          <div className="py-6 text-center text-text-muted text-sm">Loading…</div>
        )}

        {error && (
          <div className="py-6 text-center text-danger text-sm">Failed to load disks.</div>
        )}

        {disks !== null && disks.length === 0 && (
          <div className="py-6 text-center text-text-muted text-sm">No mounted volumes.</div>
        )}

        {disks !== null && disks.length > 0 && (
          <div className="space-y-2">
            {disks.map((d) => (
              <DiskRow key={d.mount_point} disk={d} />
            ))}
          </div>
        )}

        {/* Volume totals answer "how full"; this answers "full of what". */}
        <FolderTree />
      </div>
    </div>
  );
}

function DiskRow({ disk }: { disk: DiskInfo }) {
  const pct = disk.total_bytes > 0 ? (disk.used_bytes / disk.total_bytes) * 100 : 0;
  const free = disk.total_bytes - disk.used_bytes;
  const barColor =
    pct < 75
      ? 'var(--color-gpu)'
      : pct < 90
      ? 'var(--color-warning)'
      : 'var(--color-danger)';

  return (
    <div className="border border-border rounded-lg p-3 bg-bg-hover/30">
      <div className="flex items-baseline justify-between gap-2 mb-1">
        <div className="flex items-center gap-2 min-w-0">
          <span className="text-[13px] font-semibold truncate">
            {disk.name || disk.mount_point}
          </span>
          {disk.is_boot && (
            <span className="text-[11px] uppercase tracking-wider px-1.5 py-0.5 rounded-control text-text-secondary border border-border-strong">
              boot
            </span>
          )}
          {disk.is_removable && (
            <span className="text-[9px] uppercase tracking-wider px-1.5 py-0.5 rounded bg-warning/20 text-warning border border-warning/40">
              removable
            </span>
          )}
        </div>
        <span className="text-[10px] text-text-muted tabular-nums">{pct.toFixed(0)}%</span>
      </div>

      <div className="flex items-center gap-2 text-[10px] text-text-secondary mb-2">
        <span className="font-mono truncate">{disk.mount_point}</span>
        <span className="text-text-muted">•</span>
        <span className="uppercase">{disk.fs_type}</span>
      </div>

      {/* Usage bar */}
      <div className="h-2 rounded bg-bg-primary overflow-hidden mb-1.5">
        <div
          className="h-full rounded transition-[width] duration-300"
          style={{ width: `${pct}%`, backgroundColor: barColor }}
          role="progressbar"
          aria-valuenow={Math.round(pct)}
          aria-valuemin={0}
          aria-valuemax={100}
        />
      </div>

      <div className="flex items-center justify-between text-[10px] text-text-secondary tabular-nums">
        <span>
          <span className="font-semibold text-text-primary">{formatGB(disk.used_bytes, 0)}</span>
          {' '}used
        </span>
        <span>
          <span className="font-semibold text-text-primary">{formatGB(free, 0)}</span>
          {' '}free
        </span>
        <span>{formatGB(disk.total_bytes, 0)} total</span>
      </div>
    </div>
  );
}
