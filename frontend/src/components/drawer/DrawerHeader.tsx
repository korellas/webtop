interface DrawerHeaderProps {
  /** Accent colour dot (uses any CSS color, including var(--color-*)). */
  color: string;
  /** Main label (e.g. "CPU"). */
  label: string;
  /** Right-aligned live value (e.g. "47%"). Optional. */
  value?: string;
  /** Stable ID so BottomSheet's aria-labelledby can point at it. */
  labelId?: string;
  /** Invoked when the × button is clicked. */
  onClose: () => void;
}

export default function DrawerHeader({ color, label, value, labelId, onClose }: DrawerHeaderProps) {
  return (
    <div className="flex items-center justify-between gap-3 py-2 sticky top-0 bg-bg-card -mx-4 px-4 border-b border-border z-10">
      <div className="flex items-center gap-2 min-w-0">
        <span
          className="w-2.5 h-2.5 rounded-full shrink-0"
          style={{ backgroundColor: color }}
          aria-hidden="true"
        />
        <h2 id={labelId} className="text-[15px] font-semibold tracking-wide truncate">
          {label}
        </h2>
        {value !== undefined && (
          <span className="text-[13px] font-semibold tabular-nums text-text-secondary ml-1">
            {value}
          </span>
        )}
      </div>
      <button
        type="button"
        aria-label="Close"
        onClick={onClose}
        className="
          w-7 h-7 -mr-1 rounded-md
          text-text-secondary hover:text-text-primary
          hover:bg-bg-hover
          flex items-center justify-center
          transition-colors
        "
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.4" strokeLinecap="round">
          <path d="M6 6l12 12M18 6L6 18" />
        </svg>
      </button>
    </div>
  );
}
