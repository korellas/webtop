import { useMemo, useState } from 'react';
import { controlService, restartService } from '../../lib/api';
import type { ServiceStatus } from '../../lib/types';
import { useServicesStore } from '../../store/services-store';
import { useSystemStore } from '../../store/system-store';
import ServiceListItem from './ServiceListItem';
import ServiceRow from './ServiceRow';
import StackBar from './StackBar';
import { STATE_META, blockedBy, groupServices, sharedScaleMax } from './derive';
import { useRestartTracking } from './use-restart-tracking';

/** Number of `<td>`s a row emits, for the group separators' `colSpan`. */
const COLUMN_COUNT = 7;

/**
 * The services panel.
 *
 * Same table language as the process manager — sticky column headers, the same
 * row density and hover — because they are the same kind of screen and the
 * reader should not have to learn two. The headers are what make the columns
 * self-describing: a green dot is only obvious to whoever wrote it, whereas a
 * column titled "Status" needs no prior knowledge.
 *
 * Memory is drawn as bars on **one shared axis**. The first attempt normalised
 * each service against its own budget, which made 8.6/44 GB and 28.8/80 GB
 * render as almost the same fill — a chart whose bars cannot be compared to
 * each other is decoration beside a number.
 *
 * Order is manifest order, which is boot order. The panel should not rearrange
 * itself under the reader whenever a service changes state; the problem line
 * above the table is what surfaces trouble.
 */
export default function ServicesView() {
  const services = useServicesStore((s) => s.services);
  const manifestPath = useServicesStore((s) => s.manifestPath);
  const manifestError = useServicesStore((s) => s.manifestError);
  const fetchError = useServicesStore((s) => s.fetchError);
  const loaded = useServicesStore((s) => s.loaded);
  const memTotal = useSystemStore((s) => s.info?.mem_total ?? 0);

  // Which service has a confirm showing, and for which verb.
  const [armed, setArmed] = useState<{ name: string; verb: 'restart' | 'stop' } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const restarts = useRestartTracking(services);

  const groups = useMemo(() => groupServices(services), [services]);
  const scaleMax = useMemo(() => sharedScaleMax(services), [services]);
  // `starting` is deliberately in neither list: it resolves by itself, and
  // counting it would light this line up every time something restarts
  // normally.
  const faults = useMemo(
    () => services.filter((s) => s.state === 'unhealthy' || s.state === 'down'),
    [services],
  );
  const absent = useMemo(
    () => services.filter((s) => s.state === 'unregistered'),
    [services],
  );

  /**
   * `notice` is for failures only.
   *
   * It used to report success too, which meant a bar appeared at the bottom of
   * a hug-height panel and pushed everything up — a layout jump as the reward
   * for a click. Success now shows in the row that was acted on, where the
   * reader is already looking, and stays there until the PID actually changes.
   */
  async function handleRestart(s: ServiceStatus) {
    setBusy(s.name);
    setNotice(null);
    try {
      const res = await restartService(s.name);
      if (res.ok) restarts.begin(s.name, s.pid);
      else setNotice(res.message);
    } catch (e) {
      setNotice(e instanceof Error ? e.message : 'restart request failed');
    } finally {
      setBusy(null);
      setArmed(null);
    }
  }

  /**
   * Stop and start go through the control helper, which is the only thing here
   * holding any privilege. Refusals come back as `ok: false` with a reason —
   * "not in the inventory", "no sudoers rule" — and those are answers worth
   * showing rather than errors to swallow.
   */
  async function handleVerb(s: ServiceStatus, verb: 'stop' | 'start') {
    setBusy(s.name);
    setNotice(null);
    try {
      const res = await controlService(s.name, verb);
      if (!res.ok) setNotice(res.message);
    } catch (e) {
      setNotice(e instanceof Error ? e.message : `${verb} request failed`);
    } finally {
      setBusy(null);
      setArmed(null);
    }
  }

  if (manifestError) {
    return (
      <Empty
        title="No services manifest"
        detail={manifestError}
        hint={`webtop reads ${manifestPath}. Point --services-manifest at your stack's manifest, or symlink it there.`}
      />
    );
  }
  if (!loaded) return <Empty title="Loading services…" />;
  if (services.length === 0) {
    return (
      <Empty
        title="No services declared"
        hint={`${manifestPath} parsed, but contains no [[service]] entries.`}
      />
    );
  }

  return (
    <div className="flex-1 min-w-0 min-h-0 flex flex-col">
      <div className="shrink-0 px-4 py-3 border-b border-border">
        <StackBar services={services} memTotal={memTotal} />
      </div>

      {(faults.length > 0 || absent.length > 0 || fetchError) && (
        <div className="shrink-0 flex items-center gap-3 px-4 py-2 border-b border-border text-[11px]">
          {/*
            Counts, not sentences, and the border carries the tone.

            Two kinds of news share this line.
            `unhealthy` and `down` are the ones worth a colour: something that
            should be up is not. `unregistered` is a service the manifest
            declares and this machine has simply not installed — `STATE_META`
            already ranks it at the muted tone, and every row spells it out in
            its own Status cell anyway. Naming each of those in `text-danger`
            printed a wall of red above a table that then repeated the very
            same facts, and made the ordinary state of a machine that isn't
            running every declared model server look like an outage.

            Even for genuine faults a prose list is the wrong shape: it is
            frightening before it is read and redundant after, because the
            table below names every one of them in its own Status cell and
            sorts them to the top. A count is the part the header can say that
            the table cannot. The names live in the `title`.
          */}
          {faults.length > 0 && (
            <span
              className="font-mono text-[11px] px-1.5 py-0.5 rounded-control border border-danger text-danger"
              title={faults.map((s) => `${s.name} ${STATE_META[s.state].label.toLowerCase()}`).join(', ')}
            >
              {faults.length} down
            </span>
          )}
          {absent.length > 0 && (
            <span
              className="font-mono text-[11px] px-1.5 py-0.5 rounded-control border border-border text-text-muted"
              title={absent.map((s) => s.name).join(', ')}
            >
              {absent.length} not installed
            </span>
          )}
          {fetchError && (
            <span className="ml-auto text-warning" title={fetchError}>
              stale — webtop unreachable
            </span>
          )}
        </div>
      )}

      {/*
        `px-2` here plus `px-2` on the cells puts the first column's text 16 px
        from the panel edge — Carbon's cell inset, and the line the panel title
        sits on. Atlassian's scale treats cell padding (0-8 px) and container
        padding (12-24 px) as separate axes; collapsing both to 8 px is what
        made the table read as pressed against the wall.
      */}
      <div className="flex-1 min-h-0 overflow-auto thin-scroll px-2 pb-2">
        {/*
          Two layouts, not one layout with columns hidden — a 340 px phone panel
          is not a narrow table, it is not a table. See `ServiceListItem`.

          `table-fixed` with every width declared: an auto-layout table re-solves
          its columns whenever a cell's content changes, so arming a restart —
          which swaps a glyph for two buttons — shifted every column on the row
          out from under the pointer that just clicked.
        */}
        <table className="hidden sm:table table-fixed w-full border-collapse text-[11px] leading-4">
          <thead className="sticky top-0 z-10 bg-bg-sidebar">
            <tr className="text-[11px] uppercase tracking-wider text-text-secondary">
              {/* 568 px declared of the 782 px available inside the panel's
                  16 px inset, leaving ~214 px for the name — which needs 112 for
                  the longest ("model-worker :8000"). An earlier set spent 26 %
                  on a bar column and left it 79 px, truncating every model row.
                  PID's 64 px holds macOS's widest (five digits) at 11 px
                  tabular-nums plus the 16 px of cell padding. */}
              <Th label="Service" />
              <Th label="Memory" w="w-[92px]" align="right" />
              <Th label="Budget" w="w-[116px]" align="right" />
              <Th label="PID" w="w-[64px]" align="right" />
              <Th label="Up" w="w-[68px]" align="right" />
              <Th label="Status" w="w-[196px]" />
              <Th label="Actions" w="w-[32px]" srOnly />
            </tr>
          </thead>

          {groups.map(({ group, items }) => (
            <tbody key={group}>
              {/*
                Groups are separator rows rather than coloured swatches. The
                grouping is context — which part of the stack this belongs to —
                and giving it a colour put it in direct competition with status,
                the one thing on this screen that has to be noticed.
              */}
              <tr>
                <td
                  colSpan={COLUMN_COUNT}
                  className="px-2 pt-3 pb-1 text-[11px] uppercase tracking-wider text-text-muted"
                >
                  {group}
                </td>
              </tr>
              {items.map((s) => (
                <ServiceRow
                  key={s.name}
                  service={s}
                  scaleMax={scaleMax}
                  blocked={blockedBy(s, services)}
                  restartElapsed={restarts.elapsedFor(s.name)}
                  busy={busy === s.name}
                  armed={armed?.name === s.name ? armed.verb : null}
                  onArm={(verb) => setArmed(verb ? { name: s.name, verb } : null)}
                  onStop={() => handleVerb(s, 'stop')}
                  onStart={() => handleVerb(s, 'start')}
                  onRestart={() => handleRestart(s)}
                />
              ))}
            </tbody>
          ))}
        </table>

        <div className="sm:hidden">
          {groups.map(({ group, items }) => (
            <section key={group} aria-labelledby={`svc-group-${group}`}>
              <h3
                id={`svc-group-${group}`}
                className="px-2 pt-2.5 pb-1 text-[11px] uppercase tracking-wider text-text-muted"
              >
                {group}
              </h3>
              <ul>
                {items.map((s) => (
                  <ServiceListItem
                    key={s.name}
                    service={s}
                    blocked={blockedBy(s, services)}
                    restartElapsed={restarts.elapsedFor(s.name)}
                    busy={busy === s.name}
                    armed={armed?.name === s.name ? armed.verb : null}
                    onArm={(verb) => setArmed(verb ? { name: s.name, verb } : null)}
                    onStop={() => handleVerb(s, 'stop')}
                    onStart={() => handleVerb(s, 'start')}
                    onRestart={() => handleRestart(s)}
                  />
                ))}
              </ul>
            </section>
          ))}
        </div>
      </div>

      {notice && (
        <div className="shrink-0 flex items-center gap-2 px-4 py-2 border-t border-border text-[11px] text-text-secondary">
          <span className="min-w-0 truncate">{notice}</span>
          <button
            type="button"
            onClick={() => setNotice(null)}
            className="ml-auto text-text-muted hover:text-text-primary"
            aria-label="Dismiss"
          >
            ✕
          </button>
        </div>
      )}
    </div>
  );
}

/**
 * Column header, styled to match the process manager's `Th` exactly.
 *
 * No sorting here, and that is a deliberate difference rather than an
 * oversight: these rows are in manifest order, which is boot order, and that
 * ordering is information. Sorting a process list by CPU answers a question;
 * sorting eight services by memory just loses the dependency sequence.
 */
function Th({
  label,
  w,
  align,
  srOnly,
}: {
  label: string;
  w?: string;
  align?: 'right';
  srOnly?: boolean;
}) {
  return (
    <th
      className={`
        ${w ?? ''} px-2 py-2 font-semibold select-none
        border-b border-border whitespace-nowrap
        ${align === 'right' ? 'text-right' : 'text-left'}
      `}
    >
      {srOnly ? <span className="sr-only">{label}</span> : label}
    </th>
  );
}

function Empty({ title, detail, hint }: { title: string; detail?: string; hint?: string }) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-1.5 px-8 py-12 text-center">
      <span className="text-xs font-semibold text-text-secondary">{title}</span>
      {detail && <span className="text-[11px] text-danger max-w-md">{detail}</span>}
      {hint && <span className="text-[11px] text-text-muted max-w-md">{hint}</span>}
    </div>
  );
}
