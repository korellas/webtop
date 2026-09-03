//! All-user process enumeration via the system `ps`.
//!
//! Why shell out to `ps` instead of using `sysinfo`?
//!
//! On macOS, reading another user's (or root's) per-process CPU/memory via
//! `proc_pidinfo(PROC_PIDTASKINFO)` / `proc_pid_rusage` returns `EPERM`
//! unless the caller is root. `sysinfo` uses exactly that path, so when
//! webtop runs as an ordinary LaunchAgent it sees *all* PIDs but gets
//! `0` CPU and `0` memory for everything it doesn't own — those processes
//! then never make the "top-N" cut and effectively vanish from the list.
//!
//! `/bin/ps` is shipped by Apple as a hardened **setuid-root** binary
//! (`-rwsr-xr-x root wheel`), so it can read every process's stats without
//! us running the web server as root or shipping our own setuid helper.
//! We invoke it, parse the columns, and compute instantaneous CPU%
//! ourselves from the cumulative CPU-time deltas (see `metrics.rs`) — the
//! daemon stays fully unprivileged and gains zero new attack surface.
//!
//! Memory is the exception, and it goes back to `proc_pid_rusage` deliberately.
//! `ps` will hand us any process's RSS, but RSS is the wrong quantity here: it
//! does not count what a process holds through Metal, so every model server on
//! this machine reported near zero. `footprint` explains the measurement; the
//! same `EPERM` applies, so root-owned rows keep their `ps` RSS as a lower
//! bound. That is a real limit — a root daemon's Metal allocations stay
//! invisible — but it is strictly better than what RSS gave every row.

use crate::collector::snapshot::ProcessInfo;
use std::collections::{HashMap, HashSet, VecDeque};
use std::process::Command;
use std::time::Instant;

const PROCESS_WINDOW_SECS: f32 = 5.0;

#[derive(Clone)]
struct ProcessSample {
    at: Instant,
    cpu: f32,
    gpu: f32,
    mem: u64,
    name: String,
    user: String,
    cmd: String,
}

/// Stateful process sampler used only by process-related HTTP endpoints.
///
/// Keeping the cumulative counters here means the main metrics collector can
/// run without enumerating processes. CPU/GPU deltas resume naturally on the
/// next request while the process panel is open.
#[derive(Default)]
pub struct ProcessSampler {
    history: HashMap<u32, VecDeque<ProcessSample>>,
    previous_gpu_time_ns: HashMap<u32, u64>,
    previous_gpu_sample_at: Option<Instant>,
    previous_cpu_time: HashMap<u32, f64>,
    previous_cpu_sample_at: Option<Instant>,
}

impl ProcessSampler {
    pub fn sample(&mut self, system_gpu_usage: f32, total_cores: usize) -> Vec<ProcessInfo> {
        let now = Instant::now();
        let current_gpu_ns = super::gpu_procs::sample();
        let mut per_pid_gpu_pct: HashMap<u32, f32> = HashMap::new();
        if let Some(previous_at) = self.previous_gpu_sample_at {
            let elapsed = now.duration_since(previous_at).as_secs_f64().max(0.05);
            let mut raw_total_pct = 0.0;
            for (pid, current_ns) in &current_gpu_ns {
                let previous_ns = self.previous_gpu_time_ns.get(pid).copied().unwrap_or(0);
                if *current_ns >= previous_ns {
                    let delta_ns = (*current_ns - previous_ns) as f64;
                    let percent = delta_ns / (elapsed * 1_000_000_000.0) * 100.0;
                    if percent > 0.0 {
                        per_pid_gpu_pct.insert(*pid, percent as f32);
                        raw_total_pct += percent;
                    }
                }
            }
            if raw_total_pct > 0.01 && system_gpu_usage > 0.01 {
                let scale = system_gpu_usage as f64 / raw_total_pct;
                for value in per_pid_gpu_pct.values_mut() {
                    *value = (*value as f64 * scale) as f32;
                }
            }
        }
        self.previous_gpu_time_ns = current_gpu_ns;
        self.previous_gpu_sample_at = Some(now);

        let rows = sample_ps();
        let cpu_elapsed = self
            .previous_cpu_sample_at
            .map(|at| now.duration_since(at).as_secs_f64().max(0.05));
        let mut current_cpu_time = HashMap::with_capacity(rows.len());
        let total_cores = total_cores.max(1) as f32;

        let live: Vec<_> = rows
            .iter()
            .map(|row| {
                current_cpu_time.insert(row.pid, row.cpu_time_secs);
                let cpu_per_core = match (cpu_elapsed, self.previous_cpu_time.get(&row.pid)) {
                    (Some(elapsed), Some(previous)) if row.cpu_time_secs >= *previous => {
                        ((row.cpu_time_secs - previous) / elapsed * 100.0) as f32
                    }
                    _ => 0.0,
                };
                (
                    row.pid,
                    (cpu_per_core / total_cores).clamp(0.0, 100.0),
                    row.mem_bytes,
                    row.name.clone(),
                    row.user.clone(),
                    row.args.clone(),
                )
            })
            .collect();
        self.previous_cpu_time = current_cpu_time;
        self.previous_cpu_sample_at = Some(now);

        let live_pids: HashSet<u32> = live.iter().map(|(pid, ..)| *pid).collect();
        for (pid, cpu, mem, name, user, cmd) in live {
            let sample = ProcessSample {
                at: now,
                cpu,
                gpu: per_pid_gpu_pct.get(&pid).copied().unwrap_or(0.0),
                mem,
                name,
                user,
                cmd,
            };
            let samples = self.history.entry(pid).or_default();
            samples.push_back(sample);
            while samples.front().is_some_and(|front| {
                now.duration_since(front.at).as_secs_f32() > PROCESS_WINDOW_SECS
            }) {
                samples.pop_front();
            }
        }
        self.history.retain(|pid, _| live_pids.contains(pid));

        let all: Vec<ProcessInfo> = self
            .history
            .iter()
            .filter_map(|(pid, samples)| {
                let count = samples.len() as f32;
                let last = samples.back()?;
                Some(ProcessInfo {
                    pid: *pid,
                    name: last.name.clone(),
                    user: last.user.clone(),
                    cpu_percent: samples.iter().map(|sample| sample.cpu).sum::<f32>() / count,
                    gpu_percent: samples.iter().map(|sample| sample.gpu).sum::<f32>() / count,
                    mem_bytes: last.mem,
                    cmd: last.cmd.clone(),
                })
            })
            .collect();

        let mut by_cpu = all.clone();
        by_cpu.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        by_cpu.truncate(60);
        let mut by_memory = all.clone();
        by_memory.sort_by(|a, b| b.mem_bytes.cmp(&a.mem_bytes));
        by_memory.truncate(40);
        let mut by_gpu = all;
        by_gpu.sort_by(|a, b| b.gpu_percent.total_cmp(&a.gpu_percent));
        by_gpu.truncate(40);

        let mut seen = HashSet::new();
        let mut result = Vec::with_capacity(120);
        for process in by_cpu.into_iter().chain(by_gpu).chain(by_memory) {
            if seen.insert(process.pid) {
                result.push(process);
            }
        }
        result.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        result
    }
}

/// One parsed `ps` row. `cpu_time_secs` is *cumulative* CPU time consumed
/// by the process since it started; the collector diffs it across ticks to
/// derive instantaneous utilisation.
#[derive(Debug, Clone)]
pub struct PsRow {
    pub pid: u32,
    pub user: String,
    pub cpu_time_secs: f64,
    /// Resident set size as `ps` reports it. Kept as the raw parse and as the
    /// fallback for `mem_bytes`; it is not what the UI should show — see
    /// `footprint`.
    pub rss_kb: u64,
    /// What the process actually owns, in bytes: `phys_footprint` where the
    /// kernel will report it, `rss_kb` where it will not. Filled by `sample`,
    /// so a row straight out of `parse_ps_line` carries only the fallback.
    pub mem_bytes: u64,
    pub name: String,
    /// Full command line (`ps args`), truncated. Empty when unavailable.
    pub args: String,
}

/// Cap on the stored command line.
///
/// Chrome helpers and Electron apps carry multi-kilobyte argv, and we ship this
/// in every WebSocket frame, so it has to be bounded. 512 rather than something
/// tighter because the distinguishing flag is often late in the line: a model
/// tiers put `--port` after a long `--model` id and half a dozen sampler flags,
/// and a 240-char cap dropped it — leaving three servers that all displayed as
/// the same truncated model name.
const ARGS_MAX: usize = 512;

/// Sample every process via `ps`. Returns an empty Vec if `ps` can't be
/// run or produced nothing parseable (the caller treats that as "no
/// processes this tick" rather than failing the whole snapshot).
fn sample_ps() -> Vec<PsRow> {
    // `-A` = every process, `-x` is implied by `-A`. `-o … =` suppresses the
    // header for each column. `comm` is last because it (the executable path)
    // can contain spaces — everything after the 4th column is the command.
    let output = Command::new("/bin/ps")
        .args(["-Axo", "pid=,user=,time=,rss=,comm="])
        .output();

    let mut rows = match output {
        Ok(o) if o.status.success() => parse_ps_output(&String::from_utf8_lossy(&o.stdout)),
        _ => return Vec::new(),
    };

    // Command lines come from a SECOND `ps` call rather than an extra column on
    // the first. `comm` is a path that can contain spaces, so it has to be the
    // last field — append `args=` after it and there is no way to tell where one
    // ends and the other begins. Asking for `pid=,args=` separately gives an
    // unambiguous "PID, then rest of line" and costs one extra fork per process
    // refresh. Process refreshes run less often than system snapshots.
    let cmds = sample_args();
    for r in &mut rows {
        if let Some(a) = cmds.get(&r.pid) {
            r.args = a.clone();
        }
    }

    // Memory comes from the kernel's ownership ledger, not from `ps`. RSS
    // misses everything a process holds through Metal, which on this machine
    // is the entire reason the list exists: with RSS the top process on a full
    // 256 GB box read 87 MB. One `proc_pid_rusage` per row — a syscall with no
    // fork behind it: 1.7 ms for all 907 processes on this machine, 585 of
    // them readable and the rest root-owned and falling back (measured
    // 2026-08-03 through ctypes, so an upper bound). The tick it sits in
    // already includes the macmon sampling window.
    for r in &mut rows {
        r.mem_bytes = super::footprint::phys_footprint_or(r.pid, r.rss_kb.saturating_mul(1024));
    }
    rows
}

/// Cut `args` to `ARGS_MAX`, on a whitespace boundary.
///
/// Cutting mid-token silently corrupts the value: `--port 4000` became
/// `--port 400`, and a truncated path tail read as a plausible-but-wrong
/// process name. Ending on a space means the last token shown is always a whole
/// one, so anything derived from it is either right or absent.
fn truncate_args(args: &str) -> String {
    if args.len() <= ARGS_MAX {
        return args.to_string();
    }
    // `char_indices` keeps the slice on a char boundary — argv carries
    // non-ASCII (app bundle names, user paths) and byte slicing would panic.
    let hard = args
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= ARGS_MAX)
        .last()
        .unwrap_or(0);
    let cut = args[..hard].rfind(char::is_whitespace).unwrap_or(hard);
    args[..cut].to_string()
}

/// `pid → full command line`, truncated to `ARGS_MAX`.
fn sample_args() -> HashMap<u32, String> {
    let output = Command::new("/bin/ps")
        .args(["-Axo", "pid=,args="])
        .output();
    let text = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => return HashMap::new(),
    };

    text.lines()
        .filter_map(|line| {
            let mut rest = line.trim_start();
            let pid = next_token(&mut rest)?.parse::<u32>().ok()?;
            let args = rest.trim();
            if args.is_empty() {
                return None;
            }
            Some((pid, truncate_args(args)))
        })
        .collect()
}

/// Parse the whitespace-aligned `ps` output into rows. Tolerant of blank
/// lines and malformed rows (those are skipped rather than aborting).
fn parse_ps_output(text: &str) -> Vec<PsRow> {
    text.lines().filter_map(parse_ps_line).collect()
}

fn parse_ps_line(line: &str) -> Option<PsRow> {
    let mut rest = line.trim_start();
    let pid = next_token(&mut rest)?.parse::<u32>().ok()?;
    let user = next_token(&mut rest)?.to_string();
    let cpu_time_secs = parse_cpu_time(next_token(&mut rest)?);
    let rss_kb = next_token(&mut rest)?.parse::<u64>().ok()?;
    // The remainder is the executable path, which may contain spaces
    // (e.g. ".../Google Chrome Helper (Renderer)"). Use its basename so
    // the displayed name matches what users expect.
    let comm = rest.trim();
    if comm.is_empty() {
        return None;
    }
    let name = comm.rsplit('/').next().unwrap_or(comm).to_string();

    Some(PsRow {
        pid,
        user,
        cpu_time_secs,
        rss_kb,
        // The fallback, so a row is never zero-memory even if the rusage pass
        // never runs or declines. `sample` overwrites it with the footprint.
        mem_bytes: rss_kb.saturating_mul(1024),
        name,
        args: String::new(),
    })
}

/// Pop the first whitespace-delimited token off `s`, advancing `s` past any
/// following whitespace. Returns `None` when only whitespace remains.
fn next_token<'a>(s: &mut &'a str) -> Option<&'a str> {
    let t = s.trim_start();
    let end = t.find(char::is_whitespace).unwrap_or(t.len());
    if end == 0 {
        return None;
    }
    let (tok, remainder) = t.split_at(end);
    *s = remainder;
    Some(tok)
}

/// Parse a `ps` cumulative-CPU-time field into seconds.
///
/// macOS formats this as `[DD-][HH:]MM:SS.ss`, where the minutes field is
/// *unbounded* (e.g. `1011:41.34` = 1011 min 41.34 s). We parse right-to-
/// left so every layout — `SS`, `MM:SS`, `HH:MM:SS`, `DD-HH:MM:SS` — works.
fn parse_cpu_time(field: &str) -> f64 {
    let (days, rest) = match field.split_once('-') {
        Some((d, r)) => (d.parse::<f64>().unwrap_or(0.0), r),
        None => (0.0, field),
    };
    let mut secs = days * 86_400.0;
    let mut mult = 1.0;
    for part in rest.rsplit(':') {
        secs += part.parse::<f64>().unwrap_or(0.0) * mult;
        mult *= 60.0;
    }
    secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minutes_seconds() {
        assert!((parse_cpu_time("20:54.34") - (20.0 * 60.0 + 54.34)).abs() < 1e-6);
        assert!((parse_cpu_time("0:02.83") - 2.83).abs() < 1e-6);
    }

    #[test]
    fn parses_unbounded_minutes() {
        // macOS lets the minutes field exceed 60 instead of rolling to hours.
        assert!((parse_cpu_time("1011:41.34") - (1011.0 * 60.0 + 41.34)).abs() < 1e-6);
    }

    #[test]
    fn parses_hours_and_days() {
        assert!((parse_cpu_time("1:02:03") - 3723.0).abs() < 1e-6);
        assert!(
            (parse_cpu_time("2-03:04:05") - (2.0 * 86_400.0 + 3.0 * 3600.0 + 4.0 * 60.0 + 5.0))
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn parses_full_line_with_spaces_in_name() {
        let line = " 1341 alice  1011:41.34  123456 /Applications/Google Chrome.app/Contents/MacOS/Google Chrome Helper (Renderer)";
        let row = parse_ps_line(line).expect("should parse");
        assert_eq!(row.pid, 1341);
        assert_eq!(row.user, "alice");
        assert_eq!(row.rss_kb, 123456);
        assert_eq!(row.name, "Google Chrome Helper (Renderer)");
    }

    #[test]
    fn parses_root_owned_line() {
        let line = "    1 root     20:54.34   34816 /sbin/launchd";
        let row = parse_ps_line(line).expect("should parse");
        assert_eq!(row.pid, 1);
        assert_eq!(row.user, "root");
        assert_eq!(row.name, "launchd");
    }

    #[test]
    fn skips_blank_and_malformed_lines() {
        assert!(parse_ps_line("").is_none());
        assert!(parse_ps_line("   ").is_none());
        assert!(parse_ps_line("notanumber root 0:01 100 /bin/x").is_none());
    }
}
