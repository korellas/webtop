import SummaryCards from './SummaryCards';
import ChartGrid from './ChartGrid';

/**
 * The metrics screen — the dashboard's base layer, always mounted.
 *
 * It carries no chrome of its own. Machine identity, connection state and the
 * timescale selector live in the persistent bottom `StatusBar`, so this view is
 * nothing but gauges and charts and its top edge never moves.
 */
export default function SystemView() {
  return (
    <div className="flex-1 min-w-0 min-h-0 p-2 flex flex-col gap-2 overflow-auto thin-scroll">
      <SummaryCards />
      <ChartGrid />
    </div>
  );
}
