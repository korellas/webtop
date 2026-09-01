import { create } from 'zustand';

/** Keys identifying each summary chip's detail drawer. */
export type CardKey =
  | 'cpu'
  | 'ram'
  | 'disk'
  | 'net'
  | 'power'
  | 'energy';

/** Screen-space coordinate used to position the panel's pointer arrow. */
export interface AnchorPos {
  /** Horizontal center X of the clicked button (viewport coords). */
  x: number;
  /** Bottom Y of the clicked button (where the panel should start). */
  bottom: number;
}

interface DrawerState {
  openCard: CardKey | null;
  anchor: AnchorPos | null;
  open: (card: CardKey, anchor?: AnchorPos) => void;
  close: () => void;
  /** Toggle helper — closes if already open on the same card. */
  toggle: (card: CardKey, anchor?: AnchorPos) => void;
}

export const useDrawerStore = create<DrawerState>((set, get) => ({
  openCard: null,
  anchor: null,
  open: (card, anchor) => set({ openCard: card, anchor: anchor ?? null }),
  close: () => set({ openCard: null }),
  toggle: (card, anchor) => {
    const current = get().openCard;
    if (current === card) {
      set({ openCard: null });
    } else {
      set({ openCard: card, anchor: anchor ?? null });
    }
  },
}));
