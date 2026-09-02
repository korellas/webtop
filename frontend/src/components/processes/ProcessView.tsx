import { useMemo, useState } from 'react';
import { useMetricsStore } from '../../store/metrics-store';
import { useSystemStore } from '../../store/system-store';
import { formatBytes } from '../../lib/format';
import { heatBackground } from '../../lib/heat';
import type { ProcessInfo } from '../../lib/types';
import { deriveLabel } from './label';

type SortKey = 'pid' | 'name' | 'user' | 'cpu_percent' | 'gpu_percent' | 'mem_bytes';
type SortDir = 'asc' | 'desc';

/**
 * Full-screen process manager.
 *
 * Design follows the conventions of the tools people already know rather than
 * inventing something: a search field and an aggregate footer from Activity
 * Monitor, heat-tinted usage cells from Windows Task Manager, and a detail
 * panel for the selected row. The previous version was a 360 px sidebar with
 * every row weighted identically, so nothing stood out and the numbers were too
 * cramped to scan.
 */
/** `query` is owned by App so the search box can live in the overlay's title row. */
export default function ProcessView({ query }: { query: string }) {
  const snapshot = useMetricsStore((s) => s.snapshots[s.snapshots.length - 1]);
  const info = useSystemStore((s) => s.info);

  const [sortKey, setSortKey] = useState<SortKey>('cpu_percent');
  const [sortDir, setSortDir] = useState<SortDir>('desc');
  const [selected, setSelected] = useState<number | null>(null);

  const processes = useMemo(() => snapshot?.processes ?? [], [snapshot]);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? processes.filter(
          (p) =>
            p.name.toLowerCase().includes(q) ||
            String(p.pid).includes(q) ||
            (p.user ?? '').toLowerCase().includes(q) ||
            // Searching the command line is what lets you type "8002" or
            // "Quality" and land on the one model server you meant.
            (p.cmd ?? '').toLowerCase().includes(q),
        )
      : processes;

    const dir = sortDir === 'asc' ? 1 : -1;
    return [...filtered].sort((a, b) => {
      const av = a[sortKey] ?? 0;
      const bv = b[sortKey] ?? 0;
      if (typeof av === 'string' && typeof bv === 'string') return av.localeCompare(bv) * dir;
      return ((av as number) - (bv as number)) * dir;
    });
  }, [processes, query, sortKey, sortDir]);

  const totals = useMemo(
    () => ({
      cpu: rows.reduce((s, p) => s + p.cpu_percent, 0),
      mem: rows.reduce((s, p) => s + p.mem_bytes, 0),
    }),
    [rows],
  );

  const memTotal = info?.mem_total ?? 0;
  const selectedProc = rows.find((p) => p.pid === selected) ?? null;

  function toggleSort(key: SortKey) {
    if (key === sortKey) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortKey(key);
      // Text sorts read naturally A→Z; numbers are almost always "biggest
      // first" in a process list, so each column gets the direction people
      // actually want on first click instead of a single global default.
      setSortDir(key === 'name' || key === 'user' ? 'asc' : 'desc');
    }
  }

  return (
    <div className="flex-1 min-w-0 min-h-0 flex flex-col">
      <div className="flex-1 min-h-0 flex">
        {/*
          `px-2` on the scroll container, on top of the cells' own `px-2`.
          Atlassian's scale treats these as different axes — table cells sit in
          the 0-8 px band, container padding in the 12-24 px band — and
          collapsing both to 8 px is what made the table read as pressed
          against the panel wall. Together they put the first column's text 16
          px in, which is Carbon's cell inset and the same line the panel title
          sits on.
        */}
        <div className="flex-1 min-w-0 overflow-auto thin-scroll px-2 pb-2">
          {/*
            `table-fixed` with declared widths, matching the services panel.
            An auto-layout table re-solves every column whenever a cell's
            content changes, so a process whose name grows by a character
            nudges all six columns — a list that never sits still while it
            updates twice a second.

            Process is the one column with no declared width, so it gets
            whatever the others leave. That makes the declared widths a budget
            against the *narrowest* panel, not the widest: the overlay is
            `min(92vw, 800px)`, and at 390 px the desktop budget
            (60+84+64+64+92 = 364) consumed the table whole, leaving Process
            exactly 0 px — the process name, the one column the screen exists
            for, rendered at zero width with PID and User printed on top of
            each other. The numeric columns shrink below `sm` and User drops
            entirely (it reads the same account name on nearly every row of a personal
            machine), which leaves Process ~120 px.
          */}
          <table className="table-fixed w-full border-collapse text-[11px] leading-4">
            <thead className="sticky top-0 z-10 bg-bg-sidebar">
              <tr className="text-[9px] uppercase tracking-wider text-text-secondary">
                <Th w="w-[52px] sm:w-[60px]" align="right" label="PID" k="pid" {...{ sortKey, sortDir, toggleSort }} />
                <Th label="Process" k="name" {...{ sortKey, sortDir, toggleSort }} />
                <Th w="hidden sm:table-cell sm:w-[84px]" label="User" k="user" {...{ sortKey, sortDir, toggleSort }} />
                <Th w="w-[52px] sm:w-[64px]" align="right" label="CPU" k="cpu_percent" {...{ sortKey, sortDir, toggleSort }} />
                <Th w="w-[52px] sm:w-[64px]" align="right" label="GPU" k="gpu_percent" {...{ sortKey, sortDir, toggleSort }} />
                <Th w="w-[80px] sm:w-[92px]" align="right" label="Memory" k="mem_bytes" {...{ sortKey, sortDir, toggleSort }} />
              </tr>
            </thead>
            <tbody>
              {rows.map((p) => (
                <Row
                  key={p.pid}
                  p={p}
                  memTotal={memTotal}
                  selected={p.pid === selected}
                  onSelect={() => setSelected(p.pid === selected ? null : p.pid)}
                />
              ))}
              {rows.length === 0 && (
                <tr>
                  <td colSpan={6} className="text-center py-8 text-text-muted text-[11px]">
                    {query ? `No process matches "${query}"` : 'Waiting for the first sample…'}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>

        {selectedProc && <DetailPanel p={selectedProc} memTotal={memTotal} onClose={() => setSelected(null)} />}
      </div>

      {/* Aggregate footer, the way Activity Monitor does it — the totals answer
          "is anything actually eating this machine" without reading every row. */}
      <footer className="shrink-0 flex items-center gap-5 px-4 py-2 border-t border-border bg-bg-sidebar text-[10px] text-text-secondary">
        <span>
          CPU <span className="text-text-primary font-semibold tabular-nums">{totals.cpu.toFixed(1)}%</span>
        </span>
        <span>
          Memory{' '}
          <span className="text-text-primary font-semibold tabular-nums">{formatBytes(totals.mem)}</span>
          {memTotal > 0 && (
            <span className="text-text-muted"> / {formatBytes(memTotal)}</span>
          )}
        </span>
        <span className="ml-auto text-text-muted">
          Sampled every ~2s · showing the collector's top processes
        </span>
      </footer>
    </div>
  );
}

function Row({
  p, memTotal, selected, onSelect,
}: {
  p: ProcessInfo;
  memTotal: number;
  selected: boolean;
  onSelect: () => void;
}) {
  const memPct = memTotal > 0 ? (p.mem_bytes / memTotal) * 100 : 0;
  const label = deriveLabel(p.name, p.cmd ?? '');
  return (
    <tr
      onClick={onSelect}
      className={`
        cursor-pointer border-b border-border/40
        ${selected ? 'bg-bg-hover' : 'hover:bg-bg-hover/60'}
      `}
    >
      <td className="px-2 py-1.5 text-right tabular-nums text-text-muted">{p.pid}</td>
      <td className="px-2 py-1.5 truncate max-w-0">
        <span className="font-medium text-text-primary">{label.name}</span>
        {label.hint && (
          <span className="ml-1.5 text-text-muted font-normal">{label.hint}</span>
        )}
      </td>
      <td className="hidden sm:table-cell px-2 py-1.5 text-text-secondary truncate">{p.user || '—'}</td>
      <td
        className="px-2 py-1.5 text-right tabular-nums"
        style={{ background: heatBackground('var(--color-cpu)', p.cpu_percent / 100) }}
      >
        {p.cpu_percent < 0.05 ? '—' : `${p.cpu_percent.toFixed(1)}%`}
      </td>
      <td
        className="px-2 py-1.5 text-right tabular-nums"
        style={{ background: heatBackground('var(--color-gpu)', p.gpu_percent / 100) }}
      >
        {p.gpu_percent < 0.05 ? '—' : `${p.gpu_percent.toFixed(1)}%`}
      </td>
      {/* Share of RAM as a tint rather than a bar beside the number. "8.6 GB"
          means nothing on its own until you know whether the machine has 16 or
          256 GB — but the bar needed a 160 px column to say it, and the tint
          says the same thing in 92 px using the treatment CPU and GPU already
          use two cells to the left. The services panel reads identically. */}
      <td
        className="px-2 py-1.5 text-right tabular-nums text-text-primary"
        style={{ background: heatBackground('var(--color-memory)', memPct / 100) }}
      >
        {formatBytes(p.mem_bytes)}
      </td>
    </tr>
  );
}

function DetailPanel({
  p, memTotal, onClose,
}: {
  p: ProcessInfo;
  memTotal: number;
  onClose: () => void;
}) {
  const memPct = memTotal > 0 ? (p.mem_bytes / memTotal) * 100 : 0;
  return (
    <aside className="w-52 shrink-0 border-l border-border bg-bg-sidebar overflow-auto thin-scroll">
      <div className="flex items-start gap-2 px-4 py-2.5 border-b border-border">
        <div className="min-w-0">
          <div className="text-[12px] font-semibold truncate">{p.name}</div>
          <div className="text-[9px] text-text-muted">PID {p.pid}</div>
        </div>
        <button
          type="button"
          onClick={onClose}
          aria-label="Close details"
          className="ml-auto text-text-muted hover:text-text-primary text-sm leading-none"
        >
          ×
        </button>
      </div>
      <dl className="px-4 py-3 flex flex-col gap-2 text-[11px]">
        <Field label="User" value={p.user || '—'} />
        <Field label="CPU" value={`${p.cpu_percent.toFixed(2)}%`} />
        <Field label="GPU" value={`${p.gpu_percent.toFixed(2)}%`} />
        <Field label="Memory" value={`${formatBytes(p.mem_bytes)} (${memPct.toFixed(1)}%)`} />
      </dl>
      {p.cmd && (
        <div className="px-4 pb-4">
          <div className="text-[10px] text-text-muted mb-1">Command</div>
          <pre className="
            text-[9px] leading-relaxed whitespace-pre-wrap break-all
            text-text-secondary bg-bg-card border border-border rounded p-2
          ">{p.cmd}</pre>
        </div>
      )}
    </aside>
  );
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline justify-between gap-3">
      <dt className="text-text-muted text-[10px]">{label}</dt>
      <dd className="tabular-nums text-text-primary">{value}</dd>
    </div>
  );
}

function Th({
  label, k, w, align, sortKey, sortDir, toggleSort,
}: {
  label: string;
  k: SortKey;
  w?: string;
  align?: 'right';
  sortKey: SortKey;
  sortDir: SortDir;
  toggleSort: (k: SortKey) => void;
}) {
  const active = sortKey === k;
  return (
    <th
      onClick={() => toggleSort(k)}
      className={`
        ${w ?? ''} px-2 py-2 font-semibold cursor-pointer select-none
        border-b border-border whitespace-nowrap
        ${align === 'right' ? 'text-right' : 'text-left'}
        ${active ? 'text-text-primary' : 'hover:text-text-primary'}
      `}
    >
      {label}
      <span className={active ? 'ml-1' : 'ml-1 opacity-0'}>
        {sortDir === 'asc' ? '▲' : '▼'}
      </span>
    </th>
  );
}
