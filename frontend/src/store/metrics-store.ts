import { create } from 'zustand';
import type { NetworkTotals, SystemSnapshot } from '../lib/types';

const MAX_BUFFER = 3600;

interface MetricsState {
  snapshots: SystemSnapshot[];
  wsStatus: 'disconnected' | 'connecting' | 'connected';
  /**
   * Why the last history load failed, or null if it succeeded.
   *
   * Previously `loadHistory` caught its error, logged to the console, and
   * returned — leaving whatever was already in `snapshots` on screen. A failed
   * load was therefore indistinguishable from a successful one, and the user
   * read stale numbers as current. Surfacing it lets the UI say so.
   */
  historyError: string | null;
  /** Total up/down bytes for the selected timescale, or null before the
   *  first successful fetch. See `loadNetworkTotals` in `use-history`. */
  networkTotals: NetworkTotals | null;
}

interface MetricsActions {
  pushSnapshot: (snap: SystemSnapshot) => void;
  setHistory: (history: SystemSnapshot[]) => void;
  setHistoryError: (message: string | null) => void;
  setWsStatus: (status: MetricsState['wsStatus']) => void;
  setNetworkTotals: (totals: NetworkTotals) => void;
}

export const useMetricsStore = create<MetricsState & MetricsActions>()((set) => ({
  snapshots: [],
  wsStatus: 'disconnected',
  historyError: null,
  networkTotals: null,

  pushSnapshot: (snap) =>
    set((state) => ({
      snapshots:
        state.snapshots.length >= MAX_BUFFER
          ? [...state.snapshots.slice(1), snap]
          : [...state.snapshots, snap],
    })),

  setHistory: (history) => set({ snapshots: history, historyError: null }),
  setHistoryError: (historyError) => set({ historyError }),
  setWsStatus: (wsStatus) => set({ wsStatus }),
  setNetworkTotals: (networkTotals) => set({ networkTotals }),
}));
