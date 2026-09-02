import { useViewStore, type View } from '../store/view-store';
import NavIcon from './NavIcon';
import { NAV_LABELS, nextView } from './nav';

const ITEMS: View[] = ['system', 'services', 'processes'];

/**
 * The app's navigation, living in the bottom bar at every width.
 *
 * It used to be a left rail, on the reasoning that a dense height-constrained
 * dashboard can spare width more easily than height. That held on a desktop
 * and inverted on a phone, where the rail took 12.3 % of a 390 px viewport to
 * hold three icons above 780 px of empty column — so the phone got the bar and
 * the desktop kept the rail.
 *
 * Two navigations for one app is worse than either. The bar has to exist
 * anyway (connection state, timescale), and it holds these three without
 * growing, so the rail was pure additional chrome. One control, one place,
 * every width.
 *
 * Labels appear from `sm` up. There is room for them there, and a labelled tab
 * needs no prior knowledge the way a bare glyph does.
 */
export default function NavBar() {
  const view = useViewStore((s) => s.view);
  const setView = useViewStore((s) => s.setView);

  return (
    <nav aria-label="Main navigation" className="flex items-center gap-1 shrink-0">
      {ITEMS.map((item) => {
        const active = view === item;
        return (
          <button
            key={item}
            type="button"
            onClick={() => setView(nextView(view, item))}
            title={NAV_LABELS[item]}
            aria-label={NAV_LABELS[item]}
            aria-current={active ? 'page' : undefined}
            className={`
              h-7 rounded-control flex items-center gap-1.5 px-1.5 sm:px-2
              text-[11px] font-medium transition-colors
              ${active
                ? 'bg-bg-hover text-text-primary font-semibold'
                : 'text-text-secondary hover:text-text-primary hover:bg-bg-hover'}
            `}
          >
            <NavIcon view={item} size={15} />
            <span className="hidden sm:inline">{NAV_LABELS[item]}</span>
          </button>
        );
      })}
    </nav>
  );
}
