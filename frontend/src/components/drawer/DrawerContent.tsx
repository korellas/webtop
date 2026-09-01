import { lazy, Suspense } from 'react';
import DropdownPanel from './DropdownPanel';
import { useDrawerStore, type CardKey } from '../../store/drawer-store';

/**
 * Lazy-load each detail view so they're code-split out of the main bundle.
 */
const registry: Record<CardKey, React.LazyExoticComponent<React.ComponentType<DetailProps>>> = {
  cpu: lazy(() => import('./detail/CpuDetail')),
  ram: lazy(() => import('./detail/RamDetail')),
  disk: lazy(() => import('./detail/DiskDetail')),
  net: lazy(() => import('./detail/NetworkDetail')),
  power: lazy(() => import('./detail/PowerDetail')),
  energy: lazy(() => import('./detail/EnergyDetail')),
};

export interface DetailProps {
  onClose: () => void;
}

function LoadingFallback() {
  return (
    <div className="py-8 text-center text-text-muted text-sm">Loading…</div>
  );
}

export default function DrawerContent() {
  const openCard = useDrawerStore((s) => s.openCard);
  const anchor = useDrawerStore((s) => s.anchor);
  const close = useDrawerStore((s) => s.close);
  const open = openCard !== null;

  const Detail = openCard ? registry[openCard] : null;

  return (
    <DropdownPanel open={open} onClose={close} anchor={anchor} labelId="drawer-title">
      {Detail && (
        <Suspense fallback={<LoadingFallback />}>
          <Detail onClose={close} />
        </Suspense>
      )}
    </DropdownPanel>
  );
}
