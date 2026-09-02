import { useTimescaleStore } from '../store/timescale-store';
import { useChartSettingsStore } from '../store/chart-settings-store';
import { useThemeStore, type ThemePreference } from '../store/theme-store';
import type { Timescale } from '../lib/types';

const SCALES: Timescale[] = ['1m', '5m', '15m', '1h', '24h', '7d'];

/**
 * Every control in this bar is at least 24 x 24 CSS px, which is WCAG 2.2 AA's
 * minimum target size (2.5.8).
 *
 * They were not: the timescale pills measured 19 px tall, the theme buttons
 * 25 x 17. The bar's own comment below is about *horizontal* room, which is
 * genuinely scarce at 390 px — height is not. The bar is 40 px, so `min-h-7`
 * (28 px) costs nothing and is what the navigation already uses, which also
 * makes the row read as one line of controls rather than three sizes of them.
 *
 * Width is where the 390 px budget is actually spent, so the pills carry less
 * horizontal padding on a phone. `min-w-6` holds the floor underneath that —
 * `1h` and `7d` are narrow enough that padding alone would drop them under it.
 * Tightening the padding is what paid for the fullscreen button; the
 * alternative was dropping a control, and a segmented group reads fine snug.
 */
export default function TimescaleSelector() {
  const { timescale, setTimescale } = useTimescaleStore();
  const autoscale = useChartSettingsStore((s) => s.autoscale);
  const toggleAutoscale = useChartSettingsStore((s) => s.toggleAutoscale);

  return (
    <div className="flex flex-col gap-1.5 text-[10px]">
      <div className="flex items-center gap-1.5 flex-wrap">
        <div className="flex bg-bg-card border border-border rounded-md overflow-hidden">
          {SCALES.map((s) => (
            <button
              key={s}
              onClick={() => setTimescale(s)}
              className={`px-1 sm:px-2 min-h-7 min-w-6 flex items-center justify-center cursor-pointer transition-colors ${
                s === timescale
                  ? 'bg-bg-hover text-text-primary font-semibold'
                  : 'text-text-secondary hover:text-text-primary'
              }`}
            >
              {s}
            </button>
          ))}
        </div>
        <button
          onClick={toggleAutoscale}
          title="Toggle Y-axis autoscale"
          className={`px-2 min-h-7 flex items-center justify-center border rounded-md cursor-pointer transition-colors ${
            autoscale
              ? 'bg-bg-hover border-border-strong text-text-primary font-semibold'
              : 'bg-bg-card border-border text-text-secondary hover:text-text-primary'
          }`}
        >
          Auto
        </button>
        <ThemeToggle />
      </div>
    </div>
  );
}

const THEME_OPTIONS: { value: ThemePreference; label: string; icon: React.ReactNode }[] = [
  {
    value: 'light',
    label: 'Light',
    icon: (
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="12" cy="12" r="4" />
        <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
      </svg>
    ),
  },
  {
    value: 'system',
    label: 'Auto',
    icon: (
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <rect x="3" y="4" width="18" height="12" rx="2" />
        <path d="M8 20h8M12 16v4" />
      </svg>
    ),
  },
  {
    value: 'dark',
    label: 'Dark',
    icon: (
      <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
        <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
      </svg>
    ),
  },
];

/**
 * Three side-by-side choices on a desktop, one cycling button on a phone.
 *
 * Not a preference: at 390 px the bar holds the navigation, six timescale
 * pills, the autoscale toggle and this, and that adds up to ~52 px more than
 * there is. Something has to collapse, and this is the control you touch once
 * and then never again — unlike the timescale, which is how you read the
 * charts. Cycling keeps all three reachable in the space of one.
 */
function ThemeToggle() {
  const preference = useThemeStore((s) => s.preference);
  const setPreference = useThemeStore((s) => s.setPreference);

  const index = THEME_OPTIONS.findIndex((o) => o.value === preference);
  const current = THEME_OPTIONS[index] ?? THEME_OPTIONS[1];
  const next = THEME_OPTIONS[(index + 1) % THEME_OPTIONS.length];

  return (
    <>
      <button
        onClick={() => setPreference(next.value)}
        title={`Theme: ${current.label} — tap for ${next.label}`}
        aria-label={`Theme: ${current.label}. Change to ${next.label}`}
        className="
          sm:hidden flex items-center justify-center px-1 min-h-7 min-w-6
          bg-bg-card border border-border rounded-md
          text-text-secondary cursor-pointer transition-colors
        "
      >
        {current.icon}
      </button>

      <div
        className="hidden sm:flex bg-bg-card border border-border rounded-md overflow-hidden"
        role="radiogroup"
        aria-label="Theme"
      >
        {THEME_OPTIONS.map((opt) => (
          <button
            key={opt.value}
            onClick={() => setPreference(opt.value)}
            title={opt.label}
            aria-label={opt.label}
            role="radio"
            aria-checked={preference === opt.value}
            className={`px-2 min-h-7 min-w-6 cursor-pointer flex items-center justify-center transition-colors ${
              preference === opt.value
                ? 'bg-bg-hover text-text-primary'
                : 'text-text-secondary hover:text-text-primary'
            }`}
          >
            {opt.icon}
          </button>
        ))}
      </div>
    </>
  );
}
