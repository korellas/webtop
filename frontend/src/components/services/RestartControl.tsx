/**
 * Restart is two-step, because one stray click takes a 27 B model server away
 * for minutes while it re-reads its weights off disk. Not a modal, though: the
 * cost is moderate rather than grave, and a dialog on every restart would be
 * its own annoyance.
 *
 * The two steps are exported separately because the table and the phone list
 * put them in different places. In the table the confirm has to appear
 * somewhere that already reserves room for a sentence — otherwise a narrow
 * action column grows to fit "Cancel Confirm" and every other column
 * recalculates, which is the layout jumping out from under a click. In the
 * phone list there are no columns to disturb, so both steps sit together.
 */

/**
 * Height pin for controls that sit inside a table row.
 *
 * `table-fixed` pins column widths and says nothing about height, and a row is
 * as tall as its tallest cell. A padded button carrying its own line box is a
 * few pixels taller than the text it replaces, so arming a restart grew one
 * row, which grew a hug-height panel, which is centred — so the whole dialog
 * drifted by half of that in each direction. Small enough to read as a
 * rendering glitch rather than a layout bug, which is exactly why it has to be
 * pinned rather than nudged.
 *
 * `h-4` is the table's `leading-4` text line box, so both states of the cell
 * occupy the same 16 px and the row cannot move. Padding goes horizontal-only;
 * the flex centring supplies the vertical.
 *
 * The phone list opts out: 16 px is a fine click target and a poor touch one,
 * and there is no column layout to disturb there.
 */
const DENSE = 'h-4 py-0 leading-none';
const ROOMY = 'py-0.5';

interface TriggerProps {
  /** Null when the service is not running — there is nothing to signal. */
  pid: number | null;
  name: string;
  onArm: () => void;
  /** Glyph only, for the table's fixed-width action column. */
  compact?: boolean;
  /** Pin to the table's row height. See `DENSE`. */
  dense?: boolean;
}

export function RestartTrigger({ pid, name, onArm, compact, dense }: TriggerProps) {
  return (
    <button
      type="button"
      onClick={onArm}
      disabled={pid === null}
      title={pid === null ? 'Not running — nothing to signal' : `Restart ${name}`}
      aria-label={`Restart ${name}`}
      className={`
        inline-flex items-center justify-center
        px-1 rounded text-[11px] leading-none
        text-text-secondary hover:text-text-primary hover:bg-bg-hover
        disabled:opacity-30 disabled:hover:bg-transparent transition-colors
        ${dense ? DENSE : ROOMY}
      `}
    >
      {compact ? '⟳' : '⟳ Restart'}
    </button>
  );
}

interface ConfirmProps {
  name: string;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  dense?: boolean;
}

export function ConfirmPrompt({ name, busy, onCancel, onConfirm, dense }: ConfirmProps) {
  const size = dense ? DENSE : ROOMY;
  return (
    <span className="inline-flex items-center gap-1">
      <button
        type="button"
        onClick={onCancel}
        className={`
          inline-flex items-center px-1.5 rounded text-[10px] leading-none
          text-text-muted hover:text-text-primary ${size}
        `}
      >
        Cancel
      </button>
      <button
        type="button"
        onClick={onConfirm}
        disabled={busy}
        aria-label={`Confirm restart of ${name}`}
        className={`
          inline-flex items-center px-1.5 rounded text-[10px] font-semibold leading-none
          bg-danger/15 text-danger hover:bg-danger/25
          disabled:opacity-50 transition-colors ${size}
        `}
      >
        {/* Fixed-width label so the busy state cannot resize the button
            either — "…" is far narrower than "Restart". */}
        <span className="w-11 text-center">{busy ? '…' : 'Restart'}</span>
      </button>
    </span>
  );
}

interface Props extends ConfirmProps {
  pid: number | null;
  armed: boolean;
  onArm: () => void;
}

/** Both steps in one place — used where there is no column layout to disturb. */
export default function RestartControl({
  pid,
  name,
  busy,
  armed,
  onArm,
  onCancel,
  onConfirm,
}: Props) {
  return armed ? (
    <ConfirmPrompt name={name} busy={busy} onCancel={onCancel} onConfirm={onConfirm} />
  ) : (
    <RestartTrigger pid={pid} name={name} onArm={onArm} />
  );
}


/**
 * Stop when it is running, start when it is not.
 *
 * Stop is armed like restart, and for a stronger reason: a restart comes back
 * on its own, a stop stays down until someone asks for it back. Start is not
 * armed — bringing a declared service up is what the manifest already says
 * should be true, so there is nothing to second-guess.
 *
 * Deliberately absent: `disable`. It survives a reboot, and a control whose
 * effect outlives the machine's next boot does not belong one click away in a
 * dashboard anyone on the network can reach. `svc disable` is the place for it.
 */
export function PowerTrigger({
  running,
  name,
  onArm,
  onStart,
  busy,
  dense,
}: {
  running: boolean;
  name: string;
  onArm: () => void;
  onStart: () => void;
  busy: boolean;
  dense?: boolean;
}) {
  const label = running ? `Stop ${name}` : `Start ${name}`;
  return (
    <button
      type="button"
      onClick={running ? onArm : onStart}
      disabled={busy}
      title={label}
      aria-label={label}
      className={`
        inline-flex items-center justify-center
        px-1 rounded text-[11px] leading-none
        text-text-secondary hover:text-text-primary hover:bg-bg-hover
        disabled:opacity-30 disabled:hover:bg-transparent transition-colors
        ${dense ? DENSE : ROOMY}
      `}
    >
      {running ? '\u23FB' : '\u25B6'}
    </button>
  );
}

/** Confirm for stop. Same shape as the restart confirm so the row cannot move. */
export function StopConfirm({
  name,
  busy,
  onCancel,
  onConfirm,
  dense,
}: ConfirmProps) {
  const size = dense ? DENSE : ROOMY;
  return (
    <span className="inline-flex items-center gap-1">
      <button
        type="button"
        onClick={onCancel}
        className={`
          inline-flex items-center px-1.5 rounded text-[10px] leading-none
          text-text-muted hover:text-text-primary ${size}
        `}
      >
        Cancel
      </button>
      <button
        type="button"
        onClick={onConfirm}
        disabled={busy}
        aria-label={`Confirm stop of ${name}`}
        className={`
          inline-flex items-center px-1.5 rounded text-[10px] font-semibold leading-none
          bg-danger/15 text-danger hover:bg-danger/25
          disabled:opacity-50 transition-colors ${size}
        `}
      >
        <span className="w-11 text-center">{busy ? '\u2026' : 'Stop'}</span>
      </button>
    </span>
  );
}
