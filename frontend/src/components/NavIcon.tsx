import type { View } from '../store/view-store';

/** Line icons, stroked with `currentColor` so the active state is a colour swap. */
const GLYPHS: Record<View, React.ReactNode> = {
  system: (
    <>
      <path d="M3 17l4-6 4 3 5-8 5 5" />
      <path d="M3 21h18" />
    </>
  ),
  services: (
    <>
      <rect x="3" y="4" width="18" height="6" rx="1.5" />
      <rect x="3" y="14" width="18" height="6" rx="1.5" />
      <path d="M7 7h.01M7 17h.01" />
    </>
  ),
  processes: (
    <>
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <path d="M3 9h18M9 9v11" />
    </>
  ),
};

export default function NavIcon({ view, size }: { view: View; size: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {GLYPHS[view]}
    </svg>
  );
}
