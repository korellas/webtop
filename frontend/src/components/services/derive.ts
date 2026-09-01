import type { ServiceState, ServiceStatus } from '../../lib/types';

/** How each state is presented. `tone` maps to a design token, not a raw hex. */
export const STATE_META: Record<
  ServiceState,
  { label: string; tone: string; /** Ranks worst-first for sorting. */ severity: number }
> = {
  unhealthy: { label: 'Unhealthy', tone: 'var(--color-danger)', severity: 0 },
  down: { label: 'Down', tone: 'var(--color-danger)', severity: 1 },
  // Ranked above Down: a stale process holding the port blocks the real
  // service from ever starting, so it needs attention before a crash-looper.
  rogue: { label: 'Unmanaged', tone: 'var(--color-warning)', severity: 1 },
  unregistered: { label: 'Not installed', tone: 'var(--color-text-muted)', severity: 2 },
  starting: { label: 'Starting', tone: 'var(--color-warning)', severity: 3 },
  running: { label: 'Running', tone: 'var(--color-gpu)', severity: 4 },
};

/**
 * Name the dependency that explains why this service is not up.
 *
 * launchd has no ordering primitive, so a service whose dependency is missing
 * simply exits and gets retried forever. On the dashboard that reads as two
 * unrelated red cards, and the one you would go fix is not obviously the
 * cause. Naming it turns "gateway is down and so is postgresql" into "gateway
 * is waiting for postgresql" — which is the same two facts, arranged so the
 * next action is obvious.
 *
 * Only reported for services that are actually not running. A healthy service
 * whose dependency briefly blips does not need an explanation for a problem it
 * does not have.
 */
export function blockedBy(
  service: ServiceStatus,
  all: ServiceStatus[],
): string[] {
  if (service.state === 'running') return [];
  return service.depends_on.filter((dep) => {
    const d = all.find((s) => s.name === dep);
    // An unknown dependency name is a manifest typo, not a blockage; the
    // manifest is the wrong thing for this card to be complaining about.
    return d !== undefined && d.state !== 'running';
  });
}

/** What the Status column says, and why. */
export interface StatusView {
  label: string;
  tone: string;
  /** Supporting clause — the reason, or how far along. Null when there is
   *  nothing to add beyond the label. */
  detail: string | null;
}

/**
 * Resolve everything the Status column has to convey into one value.
 *
 * The interesting case is a restart in flight. For the first seconds after a
 * SIGTERM the probe still reports the old PID and a `running` state, so the
 * honest answer is not the probed state at all — it is "we asked, and we are
 * waiting". Reporting `running` there tells the user their click did nothing
 * and invites a second one.
 *
 * `starting` gets the port spelled out for the same reason: without it the
 * label means "not ready" and gives no hint whether that is normal. With it,
 * plus the uptime beside it, a two-minute weight load reads as progress rather
 * than as a hang.
 */
export function statusView(
  s: ServiceStatus,
  blocked: string[],
  restartElapsedSec: number | null,
): StatusView {
  if (restartElapsedSec !== null) {
    return {
      label: 'Restarting',
      tone: 'var(--color-warning)',
      detail: `${restartElapsedSec}s · waiting for a new pid`,
    };
  }

  const meta = STATE_META[s.state];

  if (s.state === 'starting') {
    return {
      label: meta.label,
      tone: meta.tone,
      detail:
        s.port !== null
          ? `:${s.port} not open yet · ${formatUptime(s.uptime_sec)} in`
          : `${formatUptime(s.uptime_sec)} in`,
    };
  }
  if (blocked.length > 0) {
    return { label: meta.label, tone: meta.tone, detail: `waiting for ${blocked.join(', ')}` };
  }
  if (s.state !== 'running') {
    return { label: meta.label, tone: meta.tone, detail: describeExit(s.last_exit) };
  }
  return { label: meta.label, tone: meta.tone, detail: null };
}

/** `3517` → `58m`, `90061` → `1d 1h`. */
export function formatUptime(sec: number): string {
  if (sec < 60) return `${sec}s`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

/**
 * Explain launchd's recorded exit status.
 *
 * Negative values are signal numbers. `-15` is SIGTERM, which is what our own
 * restart button sends and what a clean shutdown looks like — surfacing that
 * as a scary-looking raw number would train people to ignore the field.
 */
export function describeExit(code: number | null): string | null {
  if (code === null) return null;
  if (code === 0) return 'exited cleanly';
  if (code === -15) return 'restarted (SIGTERM)';
  if (code === -9) return 'killed (SIGKILL)';
  if (code < 0) return `killed by signal ${-code}`;
  return `exited with code ${code}`;
}

/**
 * The value one full bar width represents, shared by every row.
 *
 * A shared axis is the entire point of this layout. Normalising each row
 * against its own budget — which the earlier card grid did — made
 * `model-worker` at 8.6/44 GB and `model-large` at 28.8/80 GB render as
 * near-identical fills, hiding that one is 3.4× the other. With a common
 * denominator the bars are comparable at a glance, which is the only reason
 * to draw bars instead of printing numbers.
 *
 * The denominator is the largest budget or footprint present, not the
 * machine's total RAM. Against 256 GB even the biggest service would be a
 * stub, and the row list would stop distinguishing anything. Total RAM is
 * what the stack bar above answers.
 */
export function sharedScaleMax(services: ServiceStatus[]): number {
  const max = services.reduce(
    (m, s) => Math.max(m, s.mem_budget ?? 0, s.mem_bytes),
    0,
  );
  return max > 0 ? max : 1;
}

/** Manifest declaration order is the boot order; keep it, worst-first within a group. */
export function groupServices(
  services: ServiceStatus[],
): Array<{ group: string; items: ServiceStatus[] }> {
  const order: string[] = [];
  const byGroup = new Map<string, ServiceStatus[]>();
  for (const s of services) {
    if (!byGroup.has(s.group)) {
      byGroup.set(s.group, []);
      order.push(s.group);
    }
    byGroup.get(s.group)!.push(s);
  }
  return order.map((group) => ({ group, items: byGroup.get(group)! }));
}
