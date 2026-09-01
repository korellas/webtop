import { Component, type ErrorInfo, type ReactNode } from 'react';

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
  stack: string | null;
}

/**
 * Catch render errors and show them instead of unmounting to a blank page.
 *
 * Without this a thrown error tears down the whole React tree, leaving the
 * dark page background and nothing else — the dashboard just turns black, with
 * the actual message reachable only through the browser console. That is a bad
 * failure mode for a monitoring tool in general, and it is especially bad here
 * because webtop is the thing you open when something else is already broken:
 * a silent black screen looks exactly like the machine being wedged.
 *
 * Showing the message and stack on the page means the next failure is
 * diagnosable from a phone on the LAN, without a devtools session.
 */
export default class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, stack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    this.setState({ stack: info.componentStack ?? null });
    console.error('webtop crashed:', error, info.componentStack);
  }

  render() {
    const { error, stack } = this.state;
    if (!error) return this.props.children;

    return (
      <div className="h-dvh w-full overflow-auto bg-bg-primary text-text-primary p-6 thin-scroll">
        <div className="max-w-3xl mx-auto flex flex-col gap-4">
          <div>
            <h1 className="text-lg font-semibold text-danger">webtop crashed</h1>
            <p className="text-xs text-text-secondary mt-1">
              The dashboard hit a render error. Metrics collection is a separate
              process and is unaffected — reloading is safe.
            </p>
          </div>

          <div className="bg-bg-card border border-border rounded-lg p-3">
            <div className="text-[10px] uppercase tracking-wider text-text-muted mb-1">
              Error
            </div>
            <pre className="text-xs whitespace-pre-wrap break-words text-danger">
              {error.message || String(error)}
            </pre>
          </div>

          {stack && (
            <div className="bg-bg-card border border-border rounded-lg p-3">
              <div className="text-[10px] uppercase tracking-wider text-text-muted mb-1">
                Component stack
              </div>
              <pre className="text-[10px] leading-relaxed whitespace-pre-wrap break-words text-text-secondary">
                {stack.trim()}
              </pre>
            </div>
          )}

          <button
            type="button"
            onClick={() => window.location.reload()}
            className="self-start px-3 py-1.5 rounded-md bg-cpu text-accent-fg text-xs font-semibold hover:opacity-90 transition-opacity"
          >
            Reload
          </button>
        </div>
      </div>
    );
  }
}
