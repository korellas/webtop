import { useCallback, useEffect, useRef, useState } from 'react';
import { fetchFolders, rescanFolders, verifyFolders } from '../../../lib/api';
import { formatAgo, formatGB } from '../../../lib/format';
import type { FolderRow, FoldersResponse } from '../../../lib/types';

/**
 * "Largest folders" browser for the Disk drawer.
 *
 * A bar list rather than a treemap. Treemaps win when you need to spot
 * patterns across thousands of nodes at once; here the question is a simple
 * ranked one ("what is big"), the drawer is narrow, and at this width a
 * treemap's small rectangles lose their labels and become unclickable.
 *
 * Two-stage load, which is what makes it feel live:
 *   1. GET renders the cached tree instantly.
 *   2. A verify call re-measures the visible rows, cheapest first, within the
 *      server's 3 s budget. Rows that come back fresh flip to "now"; the rest
 *      keep showing when they were last measured.
 */
export default function FolderTree() {
  // Path stack: index 0 is the scan root, last entry is what's displayed.
  const [stack, setStack] = useState<string[]>([]);
  const [data, setData] = useState<FoldersResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [verifying, setVerifying] = useState(false);
  const [rescanning, setRescanning] = useState(false);

  const currentPath = stack.length > 0 ? stack[stack.length - 1] : undefined;

  // Guards against a slow response for a folder the user already navigated
  // away from overwriting the current view.
  const requestId = useRef(0);

  const load = useCallback(
    async (path: string | undefined, signal: AbortSignal) => {
      const id = ++requestId.current;
      const listing = await fetchFolders(path, signal);
      if (id !== requestId.current) return;

      setData(listing);
      setError(null);
      if (listing.children.length === 0) return;

      // Stage two: ask the server to re-measure what's on screen.
      setVerifying(true);
      try {
        const { updated } = await verifyFolders(
          listing.children.map((c) => c.path),
          signal,
        );
        if (id !== requestId.current || updated.length === 0) return;

        const fresh = new Map(updated.map((row) => [row.path, row]));
        setData((prev) =>
          prev === null
            ? prev
            : {
                ...prev,
                // Re-sort: a verified folder may have grown past a neighbour.
                children: prev.children
                  .map((c) => fresh.get(c.path) ?? c)
                  .sort((a, b) => b.size_bytes - a.size_bytes),
              },
        );
      } finally {
        if (id === requestId.current) setVerifying(false);
      }
    },
    [],
  );

  useEffect(() => {
    const ctrl = new AbortController();
    load(currentPath, ctrl.signal).catch((e) => {
      if (ctrl.signal.aborted) return;
      setError(String(e));
      setVerifying(false);
    });
    return () => ctrl.abort();
  }, [currentPath, load]);

  const onRescan = async () => {
    setRescanning(true);
    try {
      await rescanFolders();
    } catch {
      // A failed kick is not worth an error banner — the button just did
      // nothing and the cached tree is still valid.
    }
  };

  // While a full scan runs, poll so the tree appears when it lands.
  useEffect(() => {
    if (!rescanning && !data?.scanning) return;
    const timer = setInterval(async () => {
      try {
        const listing = await fetchFolders(currentPath);
        if (!listing.scanning) {
          setRescanning(false);
          setData(listing);
        }
      } catch {
        setRescanning(false);
      }
    }, 3000);
    return () => clearInterval(timer);
  }, [rescanning, data?.scanning, currentPath]);

  if (error) {
    return <p className="py-4 text-center text-danger text-xs">Failed to load folder sizes.</p>;
  }
  if (data === null) {
    return <p className="py-4 text-center text-text-muted text-xs">Loading…</p>;
  }

  const scanning = data.scanning || rescanning;

  if (data.never_scanned) {
    return (
      <div className="py-5 text-center">
        <p className="text-xs text-text-secondary">
          {scanning ? 'Measuring folder sizes…' : 'Folder sizes have not been measured yet.'}
        </p>
        <p className="mt-1 text-[10px] text-text-muted">
          The first scan runs a couple of minutes after startup, then every 6 hours.
        </p>
        {!scanning && (
          <button
            type="button"
            onClick={onRescan}
            className="mt-3 rounded border border-border-strong px-3 py-1 text-[11px]
                       text-text-secondary transition-colors hover:bg-bg-hover
                       hover:text-text-primary focus-visible:outline-2
                       focus-visible:outline-offset-2 focus-visible:outline-disk"
          >
            Scan now
          </button>
        )}
      </div>
    );
  }

  // Bars are scaled to the largest sibling, not to the parent total. Against
  // the parent, one dominant folder (here 204 GB of a 431 GB home) squashes
  // everything below it into indistinguishable slivers.
  const largest = data.children.reduce((max, c) => Math.max(max, c.size_bytes), 0);

  return (
    <section aria-label="Largest folders" className="mt-4 border-t border-border pt-3">
      <header className="mb-2 flex items-baseline justify-between gap-2">
        <Breadcrumb stack={stack} onNavigate={setStack} />
        <div className="flex shrink-0 items-center gap-2">
          {verifying && <span className="text-[9px] text-text-muted">checking…</span>}
          <button
            type="button"
            onClick={onRescan}
            disabled={scanning}
            title={
              data.last_full_scan_at
                ? `Full scan ${formatAgo(data.last_full_scan_at)}`
                : undefined
            }
            className="rounded px-1.5 py-0.5 text-[10px] text-text-muted transition-colors
                       hover:bg-bg-hover hover:text-text-primary disabled:opacity-40
                       focus-visible:outline-2 focus-visible:outline-offset-2
                       focus-visible:outline-disk"
          >
            {scanning ? 'scanning…' : 'rescan'}
          </button>
        </div>
      </header>

      {data.children.length === 0 ? (
        <p className="py-3 text-center text-[11px] text-text-muted">
          Nothing here is over 10&nbsp;MB.
        </p>
      ) : (
        <ul className="space-y-0.5">
          {data.children.map((row) => (
            <FolderBar
              key={row.path}
              row={row}
              largest={largest}
              onOpen={() => setStack((s) => [...s, row.path])}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

function Breadcrumb({
  stack,
  onNavigate,
}: {
  stack: string[];
  onNavigate: (next: string[]) => void;
}) {
  const crumbClass =
    'rounded px-1 py-0.5 text-[11px] text-text-secondary transition-colors ' +
    'hover:bg-bg-hover hover:text-text-primary focus-visible:outline-2 ' +
    'focus-visible:outline-offset-2 focus-visible:outline-disk';

  return (
    <nav aria-label="Folder path" className="flex min-w-0 items-baseline gap-0.5 overflow-hidden">
      <button type="button" onClick={() => onNavigate([])} className={crumbClass}>
        Home
      </button>
      {stack.map((path, i) => {
        const name = path.split('/').filter(Boolean).pop() ?? path;
        const isLast = i === stack.length - 1;
        return (
          <span key={path} className="flex min-w-0 items-baseline gap-0.5">
            <span aria-hidden className="text-text-muted">
              ›
            </span>
            {isLast ? (
              <span className="truncate px-1 text-[11px] font-semibold text-text-primary">
                {name}
              </span>
            ) : (
              <button
                type="button"
                onClick={() => onNavigate(stack.slice(0, i + 1))}
                className={`${crumbClass} truncate`}
              >
                {name}
              </button>
            )}
          </span>
        );
      })}
    </nav>
  );
}

function FolderBar({
  row,
  largest,
  onOpen,
}: {
  row: FolderRow;
  largest: number;
  onOpen: () => void;
}) {
  const width = largest > 0 ? (row.size_bytes / largest) * 100 : 0;
  const fresh = Date.now() - row.scanned_at < 10_000;

  const label =
    `${row.name}, ${formatGB(row.size_bytes)}, ` +
    `${row.file_count.toLocaleString()} files` +
    (row.has_children ? ', open' : '');

  const content = (
    <>
      <span className="w-[38%] shrink-0 truncate text-[11px] text-text-primary">{row.name}</span>

      <span className="w-16 shrink-0 text-right text-[11px] font-semibold tabular-nums">
        {formatGB(row.size_bytes)}
      </span>

      <span className="h-1.5 min-w-0 flex-1 overflow-hidden rounded-full bg-bg-primary">
        <span
          className="block h-full rounded-full bg-disk transition-[width] duration-500"
          style={{ width: `${width}%` }}
        />
      </span>

      <span
        className={`w-14 shrink-0 text-right text-[9px] tabular-nums ${
          fresh ? 'text-gpu' : 'text-text-muted'
        }`}
      >
        {formatAgo(row.scanned_at)}
      </span>
    </>
  );

  return (
    <li>
      {row.has_children ? (
        <button
          type="button"
          onClick={onOpen}
          aria-label={label}
          className="flex w-full items-center gap-2 rounded px-1 py-1 text-left
                     transition-colors hover:bg-bg-hover focus-visible:outline-2
                     focus-visible:outline-offset-2 focus-visible:outline-disk"
        >
          {content}
        </button>
      ) : (
        <div className="flex w-full items-center gap-2 px-1 py-1" aria-label={label}>
          {content}
        </div>
      )}

      {row.unreadable > 0 && (
        <p className="pl-1 text-[9px] text-warning">
          {row.unreadable} item{row.unreadable === 1 ? '' : 's'} unreadable — size is a lower bound
        </p>
      )}
    </li>
  );
}
