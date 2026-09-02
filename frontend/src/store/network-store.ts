import { create } from 'zustand';
import type { NetInterfaceInfo } from '../lib/types';

/**
 * Cached network interface list populated while the Network drawer is open.
 * Keeping the last result avoids a blank drawer on subsequent opens without
 * polling in the background.
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
