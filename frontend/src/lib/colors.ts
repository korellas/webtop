/**
 * Chart + metric palette.
 *
 * These reference the design-token CSS custom properties defined in
 * `app.css`, so the palette automatically tracks the active theme
 * (light/dark) instead of being locked to the dark-mode hex values.
 */

const v = (name: string) => `var(--color-${name})`;

export const COLORS = {
  cpu: v('cpu'),
  cpuP: v('cpu-p'),
  cpuE: v('cpu-e'),
  gpu: v('gpu'),
  memory: v('memory'),
  memorySwap: v('memory-swap'),
  memoryInactive: v('memory-inactive'),
  disk: v('disk'),
  diskWrite: v('disk-write'),
  power: v('power'),
  powerCpu: v('power-cpu'),
  powerGpu: v('power-gpu'),
  powerOther: v('power-other'),
  networkUp: v('network-up'),
  networkDown: v('network-down'),
  tempCpu: v('temp-cpu'),
  fan: v('fan'),
  danger: v('danger'),
  warning: v('warning'),

  /** Chart chrome — grid lines and axis labels. */
  chartGrid: v('chart-grid'),
  chartAxis: v('chart-axis'),
} as const;
