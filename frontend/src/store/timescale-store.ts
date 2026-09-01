import { create } from 'zustand';
import type { Timescale } from '../lib/types';

const STORAGE_KEY = 'webtop-timescale';
const VALID: Timescale[] = ['1m', '5m', '15m', '1h', '24h', '7d'];

function loadTimescale(): Timescale {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved && VALID.includes(saved as Timescale)) return saved as Timescale;
    // A retired value (notably the old '30d') is still sitting in storage from
    // a previous version. Drop it rather than leaving it to be re-rejected on
    // every load — otherwise the browser keeps a value that no longer means
    // anything and any future reader has to know the same history.
    if (saved) localStorage.removeItem(STORAGE_KEY);
  } catch { /* ignore */ }
  return '1h';
}

interface TimescaleState {
  timescale: Timescale;
  setTimescale: (ts: Timescale) => void;
}

export const useTimescaleStore = create<TimescaleState>()((set) => ({
  timescale: loadTimescale(),
  setTimescale: (timescale) => {
    try { localStorage.setItem(STORAGE_KEY, timescale); } catch { /* ignore */ }
    set({ timescale });
  },
}));
