import { create } from 'zustand';
import type { ServicesResponse, ServiceStatus } from '../lib/types';

interface ServicesState {
  services: ServiceStatus[];
  manifestPath: string;
  /** Manifest-level problem (missing file, parse error) reported by the server. */
  manifestError: string | null;
  /** Transport-level problem — the dashboard could not reach its own backend. */
  fetchError: string | null;
  loaded: boolean;

  apply: (r: ServicesResponse) => void;
  setFetchError: (message: string) => void;
}

/**
 * Kept separate from the two error kinds on purpose. "your manifest has a
 * typo" and "webtop is not answering" have nothing to do with each other, and
 * collapsing them into one string means the panel has to guess which one it is
 * showing.
 */
/**
 * Whether this build has any services to show at all.
 *
 * `null` while unknown — the nav keeps the tab until the answer is in, because
 * flashing a tab away a moment after load is worse than showing it a moment
 * too long. `false` means the manifest is missing or declares nothing, which
 * is the ordinary state of a webtop running without the stack it was written
 * beside: not an error, just a screen with nothing to put on it.
 */
export function servicesAvailable(s: {
  loaded: boolean; manifestError: string | null; services: unknown[];
}): boolean | null {
  if (!s.loaded) return null;
  return s.manifestError === null && s.services.length > 0;
}

export const useServicesStore = create<ServicesState>((set) => ({
  services: [],
  manifestPath: '',
  manifestError: null,
  fetchError: null,
  loaded: false,

  apply: (r) =>
    set({
      services: r.services,
      manifestPath: r.manifest_path,
      manifestError: r.error,
      fetchError: null,
      loaded: true,
    }),

  // The last good service list is deliberately retained. A dropped poll is
  // usually transient, and blanking the panel for one failed request would
  // make a healthy stack look like it vanished.
  setFetchError: (message) => set({ fetchError: message, loaded: true }),
}));
