import NavBar from './NavBar';
import TimescaleSelector from './TimescaleSelector';
import FullscreenToggle from './FullscreenToggle';
import { useSystemStore } from '../store/system-store';
import { useMetricsStore } from '../store/metrics-store';
import { formatBytes } from '../lib/format';

/**
 * The app's only persistent chrome: navigation, identity, connection state and
 * timescale, in one bar along the bottom.
 *
 * At the bottom rather than the top because it never changes: anchoring it
 * under the content keeps the top edge of the charts fixed no matter which
 * overlay is open, so nothing shifts vertically as you move around. A top bar
 * does the opposite — every view change happens immediately below it, and the
 * eye keeps re-finding the content's start.
 *
 * It also stays outside the overlay layer, so identity, connection state and
 * the selected timescale remain readable while a panel is open.
 *
 * 40 px and `px-3`, the same at every width. The bar previously ran at 24 px
 * on desktop with the navigation in a separate left rail; folding the rail in
 * means the content is full-bleed at every size, and the height the bar gained
 * is less than the width the rail gave back.
 */
export default function StatusBar() {
  const info = useSystemStore((s) => s.info);
  const wsStatus = useMetricsStore((s) => s.wsStatus);
  const historyError = useMetricsStore((s) => s.historyError);

  return (
    <footer
      className="
        shrink-0 z-50 h-10 flex items-center gap-2 sm:gap-3
        px-2 sm:px-3 border-t border-border bg-bg-sidebar
      "
    >
      <NavBar />

      {/* Divider, so the navigation reads as a group rather than as the first
          two of a long run of controls. Hidden on a phone, where there is no
          room for anything to its right anyway. */}
      <span className="hidden sm:block w-px h-4 bg-border shrink-0" aria-hidden />

      {/* One line, no wrapping. The bar is chrome, not content — it earns its
          height by never needing a second row, so identity collapses to a
          single `·`-joined string that truncates rather than pushing the bar
          taller. Dropped entirely on a phone, where it truncated to nothing. */}
      <span className="hidden sm:block text-[10px] text-text-secondary truncate min-w-0 leading-none">
        <span className="font-semibold text-text-primary">{info?.model ?? 'webtop'}</span>
        {/* `lg`, not `md`: at 768 px the joined string measured 256 px into a
            118 px slot, so it truncated mid-word on every tablet. */}
        {info && (
          <span className="hidden lg:inline">
            {` · ${info.chip} · ${info.p_core_count + info.e_core_count} cores · ${info.gpu_core_count}-core GPU · ${formatBytes(info.mem_total)}`}
          </span>
        )}
      </span>

      {/* `gap-2` on a phone: this group and the navigation are both `shrink-0`
          and together they claim the whole 390 px bar, so a gap here is spent
          against the last control's own width. */}
      <div className="ml-auto flex items-center gap-2 sm:gap-3 shrink-0">
        {/* Surfaced, not swallowed: a failed history load used to leave stale
            numbers on screen with the error only in the browser console. */}
        {historyError && (
          <span
            title={historyError}
            className="text-[10px] px-1.5 py-0.5 rounded bg-danger/15 text-danger font-medium"
          >
            history failed
          </span>
        )}
        <span
          className="flex items-center gap-1.5 text-[10px] text-text-secondary"
          title={`WebSocket ${wsStatus}`}
        >
          <span
            className={`w-1.5 h-1.5 rounded-full ${
              wsStatus === 'connected'
                ? 'bg-gpu'
                : wsStatus === 'connecting'
                  ? 'bg-warning'
                  : 'bg-danger'
            }`}
          />
          <span className="hidden lg:inline">{wsStatus}</span>
        </span>
        <TimescaleSelector />
        <FullscreenToggle />
      </div>
    </footer>
  );
}
