import { useEffect } from 'react';
import { create } from 'zustand';

export type ThemePreference = 'system' | 'light' | 'dark';
export type ResolvedTheme = 'light' | 'dark';

const STORAGE_KEY = 'webtop-theme';

function loadPreference(): ThemePreference {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === 'light' || saved === 'dark' || saved === 'system') return saved;
  } catch {
    /* ignore */
  }
  return 'system';
}

function systemPrefersDark(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia === 'undefined') {
    return true;
  }
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

function resolve(pref: ThemePreference): ResolvedTheme {
  if (pref === 'system') return systemPrefersDark() ? 'dark' : 'light';
  return pref;
}

interface ThemeState {
  preference: ThemePreference;
  resolved: ResolvedTheme;
  setPreference: (pref: ThemePreference) => void;
  /** Force refresh of resolved theme — called when the OS-level pref changes. */
  refresh: () => void;
}

export const useThemeStore = create<ThemeState>()((set, get) => ({
  preference: loadPreference(),
  resolved: resolve(loadPreference()),
  setPreference: (preference) => {
    try {
      localStorage.setItem(STORAGE_KEY, preference);
    } catch {
      /* ignore */
    }
    set({ preference, resolved: resolve(preference) });
  },
  refresh: () => {
    const pref = get().preference;
    set({ resolved: resolve(pref) });
  },
}));

/**
 * Hook that mirrors the resolved theme onto `document.documentElement` as a
 * `data-theme` attribute, and subscribes to OS-level scheme changes.
 * Call once near the app root.
 */
export function useTheme() {
  const resolved = useThemeStore((s) => s.resolved);
  const refresh = useThemeStore((s) => s.refresh);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', resolved);
  }, [resolved]);

  useEffect(() => {
    if (typeof window === 'undefined' || !window.matchMedia) return;
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const listener = () => refresh();
    mq.addEventListener('change', listener);
    return () => mq.removeEventListener('change', listener);
  }, [refresh]);
}
