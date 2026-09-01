import { formatBytes } from '../../lib/format';
import { heatBackground } from '../../lib/heat';
import type { ServiceStatus } from '../../lib/types';
import { ConfirmPrompt, PowerTrigger, RestartTrigger, StopConfirm } from './RestartControl';
import { formatUptime, statusView } from './derive';

/**
 * Fractions of a declared budget at which the memory reading turns urgent.
 *
 * A model server at 70 % of its budget is working as designed; at 92 % it is
 * one longer context window from swapping.
 */
const BUDGET_WARN = 0.8;
const BUDGET_DANGER = 0.92;

interface Props {
  service: ServiceStatus;
  /** Value a full-strength tint represents — the same for every row. */
  scaleMax: number;
  blocked: string[];
  /** Seconds since a restart was requested, or null if none is in flight. */
  restartElapsed: number | null;
  busy: boolean;
  /** Which confirm is showing, if any. */
  armed: 'restart' | 'stop' | null;
  onArm: (armed: 'restart' | 'stop' | null) => void;
  onStop: () => void;
  onStart: () => void;
  onRestart: () => void;
}

export default function ServiceRow({
  service: s,
  scaleMax,
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
  const budgetFraction = s.mem_budget ? s.mem_bytes / s.mem_budget : 0;

  const tone =
    budgetFraction >= BUDGET_DANGER
      ? 'var(--color-danger)'
      : budgetFraction >= BUDGET_WARN
        ? 'var(--color-warning)'
        : 'var(--color-memory)';

  return (
    <tr className="border-b border-border/40 hover:bg-bg-hover/60">
      <td className="px-2 py-1.5 truncate">
        <span className="font-medium text-text-primary">{s.name}</span>
        {/* Port folded into the name rather than given a column — four
            characters that only mean anything next to the name. */}
        {s.port !== null && (
          <span className="ml-1.5 text-text-muted tabular-nums">:{s.port}</span>
        )}
      </td>

      {/*
        Magnitude as a cell tint on the axis shared by every row, which is what
        the bar column used to carry. That column cost 26 % of the panel and
        was the direct reason the service name had 79 px to render in; the tint
        says the same thing in none, using the treatment the process manager
        already applies to CPU, GPU and memory.
      */}
      <td
        className="px-2 py-1.5 text-right font-semibold tabular-nums"
        style={{ background: heatBackground(tone, s.mem_bytes / scaleMax) }}
      >
        {formatBytes(s.mem_bytes)}
      </td>

      <td className="px-2 py-1.5 text-right tabular-nums text-text-muted">
        {s.mem_budget !== null
          ? `${Math.round(budgetFraction * 100)}% of ${formatBytes(s.mem_budget)}`
          : '—'}
      </td>

      {/*
        The PID is here because it is the only unambiguous evidence a restart
        happened. Every other cell survives one: a model server can re-read its weights from
        a warm mmap in twelve seconds, so a poll or two later the row says
        Running at the same memory again and the click reads as having done
        nothing. A number that changed says otherwise.
      */}
      <td className="px-2 py-1.5 text-right tabular-nums text-text-muted">
        {s.pid ?? '—'}
      </td>

      <td className="px-2 py-1.5 text-right tabular-nums text-text-secondary">
        {s.pid === null ? '—' : formatUptime(s.uptime_sec)}
      </td>

      {/*
        The confirm step lives here rather than in the action column, and the
        column widths are fixed. Between them that is what stops the table
        shifting under the pointer: "Cancel Restart" is far wider than the ⟳
        glyph, so in an auto-layout table with a snug action column, arming a
        restart re-solved every column on the row.

        Status is also the honest place for it — what this row is about to be
        is what the status column is for.
      */}
      {/*
        Both of these cells wrap their contents in a fixed-height flex box.
        `h-4` on the buttons alone was not enough: a button is inline-level, so
        it sits on the text baseline and the line box reserves room for the
        descender beneath it — measured 17.5 px at rest and 20 px armed, for a
        control that is 16 px tall either way. Flex has no baseline to sit on,
        so the box is exactly its declared height and the row cannot breathe.
      */}
      <td className="px-2 py-1.5">
        <div className="flex items-center h-4 min-w-0">
          {armed === 'restart' ? (
            <ConfirmPrompt
              name={s.name}
              busy={busy}
              onCancel={() => onArm(null)}
              onConfirm={onRestart}
              dense
            />
          ) : armed === 'stop' ? (
            <StopConfirm
              name={s.name}
              busy={busy}
              onCancel={() => onArm(null)}
              onConfirm={onStop}
              dense
            />
          ) : (
            <>
              <span className="font-medium shrink-0" style={{ color: status.tone }}>
                {status.label}
              </span>
              {status.detail && (
                <span className="ml-1.5 text-text-muted truncate" title={status.detail}>
                  {status.detail}
                </span>
              )}
            </>
          )}
        </div>
      </td>

      <td className="px-1 py-1.5">
        {/* Kept mounted while armed so the cell's contents never change width;
            invisible but still occupying its box. */}
        <div className={`flex items-center justify-end gap-0.5 h-4 ${armed ? 'invisible' : ''}`}>
          <PowerTrigger
            running={s.pid !== null}
            name={s.name}
            onArm={() => onArm('stop')}
            onStart={onStart}
            busy={busy}
            dense
          />
          <RestartTrigger pid={s.pid} name={s.name} onArm={() => onArm('restart')} compact dense />
        </div>
      </td>
    </tr>
  );
}
