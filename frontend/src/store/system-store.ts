import { create } from 'zustand';
import type { SystemInfo } from '../lib/types';

interface SystemState {
  info: SystemInfo | null;
  setInfo: (info: SystemInfo) => void;
}

export const useSystemStore = create<SystemState>()((set) => ({
  info: null,
  setInfo: (info) => set({ info }),
}));
