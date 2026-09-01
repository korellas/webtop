import { create } from 'zustand';

/** Top-level screens, selected from the left rail. */
export type View = 'system' | 'services' | 'processes';

const VALID: View[] = ['system', 'services', 'processes'];
const STORAGE_KEY = 'webtop-view';

/**
 * Read the view from the URL hash first, then localStorage.
 *
 * The hash wins so a bookmarked or shared `#/processes` lands where it says.
 * No router: this is one binary serving one SPA, and react-router would add a
 * dependency plus SPA-fallback complexity to replace about ten lines.
 */
function loadView(): View {
  const fromHash = window.location.hash.replace(/^#\/?/, '') as View;
  if (VALID.includes(fromHash)) return fromHash;
  try {
    const saved = localStorage.getItem(STORAGE_KEY) as View | null;
    if (saved && VALID.includes(saved)) return saved;
    if (saved) localStorage.removeItem(STORAGE_KEY);
  } catch { /* private browsing — fall through to the default */ }
  return 'system';
}

interface ViewState {
  view: View;
  setView: (v: View) => void;
}

export const useViewStore = create<ViewState>()((set) => ({
  view: loadView(),
  setView: (view) => {
    try { localStorage.setItem(STORAGE_KEY, view); } catch { /* ignore */ }
    // `replaceState`, not a hash assignment: writing `location.hash` pushes a
    // history entry, so flipping between tabs would stack up entries and the
    // back button would walk the tab history instead of leaving the dashboard.
    window.history.replaceState(null, '', `#/${view}`);
    set({ view });
  },
}));
