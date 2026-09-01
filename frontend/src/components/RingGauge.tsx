interface RingGaugeProps {
  value: number;
  max?: number;
  color: string;
  size?: number;
  /**
   * Optional track (unfilled arc) stroke. Defaults to the
   * theme-aware `--color-border` token so it blends with both
   * dark and light palettes.
   */
  track?: string;
  strokeWidth?: number;
}

export default function RingGauge({
  value,
  max = 100,
  color,
  size = 36,
  track = 'var(--color-border-strong)',
  strokeWidth = 3,
}: RingGaugeProps) {
  const pct = Math.min(Math.max(value / max, 0), 1);
  const r = 15;
  const circumference = 2 * Math.PI * r;
  // Give a tiny minimum so hovering-near-zero values still show a dot
  // (otherwise the filled arc is a zero-length dash, invisible even at
  // strokeLinecap="round" in some renderers).
  const filled = pct > 0 ? Math.max(circumference * pct, 0.8) : 0;
  const empty = Math.max(circumference - filled, 0);

  return (
    <svg
      viewBox="0 0 36 36"
      width={size}
      height={size}
      style={{ transform: 'rotate(-90deg)' }}
    >
      {/*
        `style` is used for stroke instead of the SVG attribute so CSS
        custom properties (`var(--color-border)`) are always honoured
        by the CSS engine, independent of SVG-attribute-level var()
        support quirks.
      */}
      <circle
        cx="18"
        cy="18"
        r={r}
        fill="none"
        style={{ stroke: track }}
        strokeWidth={strokeWidth}
      />
      <circle
        cx="18"
        cy="18"
        r={r}
        fill="none"
        style={{ stroke: color }}
        strokeWidth={strokeWidth}
        strokeDasharray={`${filled} ${empty}`}
        strokeLinecap="round"
      />
    </svg>
  );
}
