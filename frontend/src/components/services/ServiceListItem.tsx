import { formatBytes } from '../../lib/format';
import type { ServiceStatus } from '../../lib/types';
import RestartControl, { PowerTrigger, StopConfirm } from './RestartControl';
import { formatUptime, statusView } from './derive';

interface Props {
  service: ServiceStatus;
  blocked: string[];
  /** Seconds since a restart was requested, or null if none is in flight. */
  restartElapsed: number | null;
  busy: boolean;
  armed: 'restart' | 'stop' | null;
  onArm: (armed: 'restart' | 'stop' | null) => void;
  onStop: () => void;
  onStart: () => void;
  onRestart: () => void;
}

/**
 * One service, laid out for a phone.
 *
 * Deliberately not the table row with columns hidden. In portrait the panel is
 * roughly 340 px wide; after a name, a value and a status there is nothing left
 * to hide, and what survives is a table squeezed until the name truncates to
 * three characters. Two lines of full-width text fit the same facts
 * comfortably.
 *
 * The shared-axis bar is gone here rather than shrunk. Its whole purpose is
 * comparing services against each other, and a 60 px track cannot do that — it
 * would just be a restless horizontal element in a vertical list, saying less
 * than the number printed beside it.
 */
export default function ServiceListItem({
  service: s,
  blocked,
  restartElapsed,
  busy,
  armed,
  onArm,
  onStop,
  onStart,
  onRestart,
}: Props) {
  const status = statusView(s, blocked, restartElapsed);

  const facts = [
    formatBytes(s.mem_bytes),
    s.mem_budget !== null
      ? `${Math.round((s.mem_bytes / s.mem_budget) * 100)}% of ${formatBytes(s.mem_budget)}`
      : null,
    s.pid !== null ? `up ${formatUptime(s.uptime_sec)}` : null,
    // Last, so it is the first thing dropped when the line truncates: on a
    // phone the memory reading is what you came for. See `ServiceRow` for why
    // it is on screen at all.
    s.pid !== null ? `pid ${s.pid}` : null,
  ].filter(Boolean);

  return (
    <li className="flex flex-col gap-0.5 px-4 py-2.5 border-b border-border/40">
      <div className="flex items-baseline gap-1.5 min-w-0">
        <span className="text-[13px] font-medium truncate">{s.name}</span>
        {s.port !== null && (
          <span className="text-[11px] text-text-muted tabular-nums shrink-0">:{s.port}</span>
        )}
        <span
          className="ml-auto text-[11px] font-medium shrink-0"
          style={{ color: status.tone }}
        >
          {status.label}
        </span>
      </div>

      <div className="flex items-baseline gap-1.5 min-w-0">
        <span className="text-[11px] text-text-secondary tabular-nums truncate">
          {facts.join(' · ')}
        </span>
        {/* Both steps sit together here — there are no columns to disturb,
            which is why the table splits them and this list does not. */}
        <span className="ml-auto shrink-0 inline-flex items-center gap-1">
          {armed === 'stop' ? (
            <StopConfirm
              name={s.name}
              busy={busy}
              onCancel={() => onArm(null)}
              onConfirm={onStop}
            />
          ) : (
            <>
              <PowerTrigger
                running={s.pid !== null}
                name={s.name}
                onArm={() => onArm('stop')}
                onStart={onStart}
                busy={busy}
              />
              <RestartControl
                pid={s.pid}
                name={s.name}
                busy={busy}
                armed={armed === 'restart'}
                onArm={() => onArm('restart')}
                onCancel={() => onArm(null)}
                onConfirm={onRestart}
              />
            </>
          )}
        </span>
      </div>

      {status.detail && (
        <span className="text-[11px]" style={{ color: status.tone }}>
          {status.detail}
        </span>
      )}
    </li>
  );
}
