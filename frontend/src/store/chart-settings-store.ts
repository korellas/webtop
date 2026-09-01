import { create } from 'zustand';

const STORAGE_KEY = 'webtop-chart-autoscale';

function loadAutoscale(): boolean {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === '1') return true;
    if (saved === '0') return false;
  } catch {
    /* ignore */
  }
  return false;
}

interface ChartSettingsState {
  autoscale: boolean;
  toggleAutoscale: () => void;
  setAutoscale: (v: boolean) => void;
}

export const useChartSettingsStore = create<ChartSettingsState>()((set, get) => ({
  autoscale: loadAutoscale(),
  toggleAutoscale: () => {
    const next = !get().autoscale;
    try {
      localStorage.setItem(STORAGE_KEY, next ? '1' : '0');
    } catch {
      /* ignore */
    }
    set({ autoscale: next });
  },
  setAutoscale: (autoscale) => {
    try {
      localStorage.setItem(STORAGE_KEY, autoscale ? '1' : '0');
    } catch {
      /* ignore */
    }
    set({ autoscale });
  },
}));
