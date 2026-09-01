import { create } from 'zustand';
import type { NetInterfaceInfo } from '../lib/types';

/**
 * Cached network interface list — populated by `useNetworkPoll()` which
 * runs from `App.tsx` from the moment the page loads. This means the
 * NetworkDetail drawer has data ready to render the instant the user
 * opens it; no per-open fetch, no spinner.
 */
interface NetworkState {
  interfaces: NetInterfaceInfo[] | null;
  /** True once at least one fetch has returned (success or failure). */
  loaded: boolean;
  setInterfaces: (ifs: NetInterfaceInfo[]) => void;
  markLoaded: () => void;
}

export const useNetworkStore = create<NetworkState>((set) => ({
  interfaces: null,
  loaded: false,
  setInterfaces: (ifs) => set({ interfaces: ifs, loaded: true }),
  markLoaded: () => set({ loaded: true }),
}));
