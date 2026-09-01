/**
 * Turn a command line into something that tells two same-named processes apart.
 *
 * The executable name alone is often useless on a machine like this one: four
 * `python3.1` rows are four different model servers, and `node` could be any of
 * a dozen things. The command line always distinguishes them — the trick is
 * showing the distinguishing part without dumping 240 characters into a table
 * cell.
 *
 * The rules below are deliberately generic. Encoding "port 8001 means
 * model-worker" here would make the dashboard know about one specific stack;
 * showing `:8001` lets the reader make that connection themselves, and works
 * the same for anything else that binds a port.
 */

/** Flags whose *value* is worth showing, in priority order. */
const INTERESTING_FLAGS = ['--port', '-p', '--model', '--config'];

export interface ProcessLabel {
  /** Executable name, unchanged. */
  name: string;
  /** Short distinguishing suffix, or empty when the name already stands alone. */
  hint: string;
}

export function deriveLabel(name: string, cmd: string): ProcessLabel {
  if (!cmd) return { name, hint: '' };

  const tokens = cmd.split(/\s+/);
  const parts: string[] = [];

  // A subcommand — `model serve`, `git status`. The first bare word after the
  // executable is usually the single most informative token in the whole line.
  const sub = tokens[1];
  if (sub && !sub.startsWith('-') && !sub.includes('/') && sub.length <= 16) {
    parts.push(sub);
  }

  for (const flag of INTERESTING_FLAGS) {
    const i = tokens.indexOf(flag);
    if (i === -1) continue;
    const raw = tokens[i + 1];
    if (!raw || raw.startsWith('-')) continue;
    if (flag === '--port' || flag === '-p') {
      parts.push(`:${raw}`);
    } else {
      // Model ids and config paths are long and mostly shared prefix; the tail
      // is the part that differs between two instances.
      const tail = raw.split('/').pop() ?? raw;
      parts.push(tail.length > 22 ? `…${tail.slice(-20)}` : tail);
    }
    break; // one qualifier is enough for a table cell
  }

  // Nothing structured to show — fall back to the script or bundle being run,
  // which is what makes one `node` or `python` different from the next.
  if (parts.length === 0) {
    const pathish = tokens
      .slice(1)
      .find((t) => t.includes('/') && !t.startsWith('-'));
    if (pathish) {
      const base = pathish.split('/').pop();
      if (base && base !== name) parts.push(base);
    }
  }

  return { name, hint: parts.join(' ') };
}
