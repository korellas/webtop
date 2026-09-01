//! Measuring what a declared service is actually doing right now.
//!
//! Three independent sources are joined per service:
//!
//! | source | one call for all services | answers |
//! |---|---|---|
//! | `launchctl print system` | yes | is it registered, what PID, how did it last exit |
//! | `ps -Ao pid,ppid,rss,%cpu,etime` | yes | the process tree's shape, CPU, age |
//! | `proc_pid_rusage` | one per pid in a tree | what each process actually owns |
//! | TCP connect to `127.0.0.1:<port>` | one per service | is it actually serving |
//!
//! The first two are single process spawns regardless of service count
//! (measured: `launchctl print system` at 14 ms), which is what makes a 5 s
//! sampling cadence cheap enough to run forever.

use serde::Serialize;
use std::collections::HashMap;
use std::io;
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

/// How long a service may sit with its PID up but its port closed before we
/// stop calling it "starting" and start calling it unhealthy.
///
/// Sized for the worst legitimate case on this machine: the 27B model server
/// reads ~80 GB of weights off disk before it binds. Anything past ten minutes
/// is not loading, it is stuck.
const STARTUP_GRACE_SEC: u64 = 600;

/// Budget for the liveness connect. Loopback either answers immediately or is
/// not listening; a long timeout would only serve to stall the whole sampling
/// pass behind one sick service.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    /// Process is up and, if it declares a port, that port accepts connections.
    Running,
    /// Process is up but its port is not open yet, and it has not been up long
    /// enough for that to be alarming.
    Starting,
    /// Process is up, port is closed, and the grace period has expired. This is
    /// the state worth an alert: launchd believes everything is fine, so
    /// nothing else on the machine will tell you.
    Unhealthy,
    /// Registered with launchd but no process. Under `KeepAlive` this should be
    /// a blink between restarts; if it persists, the job is crash-looping.
    Down,
    /// In the manifest but not registered with launchd at all — the installer
    /// has not been run since this service was declared.
    Unregistered,
    /// Not registered with launchd, yet its port is serving: something is
    /// running this service outside the supervisor.
    ///
    /// Worth separating from `Unregistered` because the two call for opposite
    /// reactions. Unregistered is an absence — apply the manifest and it is
    /// solved. This is a *conflict*: applying the manifest now starts a second
    /// instance that cannot bind the port, so the process holding it has to go
    /// first. It also survives nothing — a reboot leaves the port dead, since
    /// launchd was never told about the job.
    ///
    /// Detected here only in its simplest shape (no job at all, port open).
    /// The nastier one — a live launchd job whose port is held by a process
    /// outside its tree — needs the listener's PID, which this probe does not
    /// collect; `svc doctor` compares port ownership and reports it.
    Rogue,
}

/// One process tree's resource usage.
#[derive(Debug, Clone, Copy, Default)]
struct TreeUsage {
    mem_bytes: u64,
    cpu_percent: f32,
    proc_count: u32,
    /// Age of the tree's root, in seconds.
    uptime_sec: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    pub name: String,
    pub label: String,
    pub port: Option<u16>,
    pub group: String,
    pub mem_budget: Option<u64>,
    pub depends_on: Vec<String>,

    pub state: ServiceState,
    /// launchd's PID for the job — the *root* of the tree, which for a
    /// shell-launched service is the shell, not the thing using the memory.
    pub pid: Option<u32>,
    /// Exit status launchd recorded for the last run. Negative values are
    /// signal numbers (`-15` = SIGTERM, i.e. a clean restart).
    pub last_exit: Option<i32>,
    pub port_open: bool,
    /// Summed across the entire process tree — see `tree_usage`.
    pub mem_bytes: u64,
    pub cpu_percent: f32,
    pub proc_count: u32,
    pub uptime_sec: u64,
}

/// What launchd knows about one job.
#[derive(Debug, Clone, Copy)]
pub struct LaunchdEntry {
    /// `None` when launchd reports PID 0, meaning registered but not running.
    pid: Option<u32>,
    last_exit: Option<i32>,
}

/// Parse the `services = { … }` block of `launchctl print system`.
///
/// Each row is `<pid> <status> <label>`, where a PID of 0 means "registered,
/// not running" and a status of `-` means "has not exited". One call covers
/// every job on the machine, so per-service cost is a hash lookup.
pub fn launchd_states() -> HashMap<String, LaunchdEntry> {
    let out = Command::new("launchctl").args(["print", "system"]).output();
    let Ok(out) = out else {
        return HashMap::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    parse_launchd_services(&text)
}

fn parse_launchd_services(text: &str) -> HashMap<String, LaunchdEntry> {
    let mut map = HashMap::new();
    let mut in_services = false;

    for line in text.lines() {
        let t = line.trim();
        if !in_services {
            // The block we want; `launchctl print` also emits `endpoints =`,
            // `disabled services =` and others with different row shapes.
            if t == "services = {" {
                in_services = true;
            }
            continue;
        }
        if t == "}" {
            break;
        }

        let mut fields = t.split_whitespace();
        let (Some(pid), Some(status), Some(label)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        // A label containing whitespace would have been split; there is no
        // such thing, but guarding costs one comparison and avoids attributing
        // a row to a truncated label.
        if fields.next().is_some() {
            continue;
        }

        let pid = pid.parse::<u32>().ok().filter(|p| *p != 0);
        let last_exit = status.parse::<i32>().ok();
        map.insert(label.to_string(), LaunchdEntry { pid, last_exit });
    }
    map
}

/// One `ps` pass over every process, indexed for subtree walks.
struct ProcTable {
    children: HashMap<u32, Vec<u32>>,
    /// Physical footprint where the kernel reports it, `ps` RSS where it does
    /// not — see `collector::footprint`. Not RSS: an MLX server read 2.3 GB by
    /// RSS while owning 59.8 GB, which made the budget gauge useless.
    mem_bytes: HashMap<u32, u64>,
    cpu: HashMap<u32, f32>,
    uptime_sec: HashMap<u32, u64>,
}

/// `ps` elapsed time — `[[dd-]hh:]mm:ss` — as seconds.
fn parse_etime(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let mut parts = rest.split(':').rev();
    let secs: u64 = parts.next()?.parse().ok()?;
    let mins: u64 = parts.next().unwrap_or("0").parse().ok()?;
    let hours: u64 = parts.next().unwrap_or("0").parse().ok()?;
    Some(days * 86_400 + hours * 3_600 + mins * 60 + secs)
}

fn proc_table() -> ProcTable {
    let mut table = ProcTable {
        children: HashMap::new(),
        mem_bytes: HashMap::new(),
        cpu: HashMap::new(),
        uptime_sec: HashMap::new(),
    };

    // Every selected field is whitespace-free, so positional splitting is
    // safe. `comm`/`args` are deliberately absent for exactly that reason —
    // they contain spaces and would have to be last, which `processes.rs`
    // handles with a second call it needs and this one does not.
    let Ok(out) = Command::new("/bin/ps")
        .args(["-Ao", "pid=,ppid=,rss=,%cpu=,etime="])
        .output()
    else {
        return table;
    };

    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        let (Ok(pid), Ok(ppid)) = (f[0].parse::<u32>(), f[1].parse::<u32>()) else {
            continue;
        };
        // ps reports RSS in kilobytes, and it is only the fallback: the
        // footprint is what the budget gauge is denominated in.
        let rss_bytes = f[2].parse::<u64>().unwrap_or(0).saturating_mul(1024);
        table.mem_bytes.insert(
            pid,
            crate::collector::footprint::phys_footprint_or(pid, rss_bytes),
        );
        table.cpu.insert(pid, f[3].parse::<f32>().unwrap_or(0.0));
        table.uptime_sec.insert(pid, parse_etime(f[4]).unwrap_or(0));
        table.children.entry(ppid).or_default().push(pid);
    }
    table
}

/// Sum memory and CPU over a service's **entire** process tree.
///
/// This is not an optimisation, it is the difference between a right and a
/// wrong number. launchd's PID for a model worker is the wrapper shell, which
/// holds 2 MB; the 8.6 GB of model weights live in a Python process two levels
/// below it (shell → shell → python, measured 2026-08-01). Reading the root
/// PID's memory — or even its direct children's — would under-report by three
/// orders of magnitude, and the memory-budget gauge that motivates this whole
/// panel would read zero on a service holding a third of the machine.
///
/// Walking the tree is necessary but was not sufficient: the per-process
/// quantity has to be the physical footprint too. With `ps` RSS this walk
/// summed a model worker to 2.84 GB against its declared 44 GB while the
/// process actually owned 59.8 GB — 16 GB *over* budget, presented as 6 % of
/// it. See `collector::footprint` for what RSS misses and what the footprint
/// still cannot see.
fn tree_usage(table: &ProcTable, root: u32) -> TreeUsage {
    let mut usage = TreeUsage {
        uptime_sec: table.uptime_sec.get(&root).copied().unwrap_or(0),
        ..Default::default()
    };

    // Iterative walk with a visited set. A PID cycle cannot happen on a sane
    // system, but `ps` is a non-atomic snapshot of a mutating table and an
    // unbounded loop here would hang the sampler thread forever.
    let mut seen = std::collections::HashSet::new();
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        usage.proc_count += 1;
        usage.mem_bytes += table.mem_bytes.get(&pid).copied().unwrap_or(0);
        usage.cpu_percent += table.cpu.get(&pid).copied().unwrap_or(0.0);
        if let Some(kids) = table.children.get(&pid) {
            stack.extend(kids);
        }
    }
    usage
}

/// Can something be reached on this port?
///
/// Deliberately a connect-and-drop rather than an HTTP request. A model server
/// mid-weight-load has its port bound but answers nothing useful, and an HTTP
/// health check would need per-service knowledge of what a healthy response
/// looks like — knowledge that lives in the stack, not in webtop. "Accepts TCP"
/// is the strongest liveness signal available generically, and paired with the
/// startup grace period it separates loading from stuck.
fn port_open(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
        Ok(s) => {
            let _ = s.shutdown(Shutdown::Both);
            true
        }
        Err(_) => false,
    }
}

/// Probe every service in `defs` and return their current status.
pub fn probe_all(defs: &[super::manifest::ServiceDef]) -> Vec<ServiceStatus> {
    let launchd = launchd_states();
    let table = proc_table();

    defs.iter()
        .map(|def| {
            let entry = launchd.get(&def.label);
            let pid = entry.and_then(|e| e.pid);
            let usage = pid.map(|p| tree_usage(&table, p)).unwrap_or_default();
            let open = match (pid, def.port) {
                (Some(_), Some(port)) => port_open(port),
                _ => false,
            };

            let state = match (entry, pid) {
                (None, _) if open => ServiceState::Rogue,
                (None, _) => ServiceState::Unregistered,
                (Some(_), None) => ServiceState::Down,
                // No declared port: the process existing is all we can check.
                (Some(_), Some(_)) if def.port.is_none() => ServiceState::Running,
                (Some(_), Some(_)) if open => ServiceState::Running,
                (Some(_), Some(_)) if usage.uptime_sec < STARTUP_GRACE_SEC => {
                    ServiceState::Starting
                }
                _ => ServiceState::Unhealthy,
            };

            ServiceStatus {
                name: def.name.clone(),
                label: def.label.clone(),
                port: def.port,
                group: def.group.clone(),
                mem_budget: def.mem_budget,
                depends_on: def.depends_on.clone(),
                state,
                pid,
                last_exit: entry.and_then(|e| e.last_exit),
                port_open: open,
                mem_bytes: usage.mem_bytes,
                cpu_percent: usage.cpu_percent,
                proc_count: usage.proc_count,
                uptime_sec: usage.uptime_sec,
            }
        })
        .collect()
}

/// Ask a service to restart by signalling its process tree root.
///
/// There is no "start" or "stop" counterpart and that is deliberate: the jobs
/// live in launchd's `system/` domain, which needs root to control, and
/// `KeepAlive` resurrects anything that exits — so a stop button would be a
/// button that does nothing for two seconds. Restart needs neither, because
/// the daemons run as this same uid, so an unprivileged `SIGTERM` is
/// permitted and `KeepAlive` doing its job *is* the restart.
pub fn restart(status: &ServiceStatus) -> Result<(), String> {
    let Some(pid) = status.pid else {
        return Err(format!("{} is not running", status.name));
    };
    // SAFETY: kill() takes two integers, touches no memory we own, and reports
    // failure through errno rather than by any unsound path.
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if rc == 0 {
        return Ok(());
    }
    Err(format!(
        "could not signal {} (pid {pid}): {}",
        status.name,
        io::Error::last_os_error()
    ))
}

/// How long a service may keep running after SIGTERM before we escalate to
/// SIGKILL.
///
/// This is the two-phase stop every supervisor implements — systemd's
/// `TimeoutStopSec`, launchd's `ExitTimeOut`, supervisord's `stopwaitsecs`:
/// give a clean shutdown a bounded chance, then force it so launchd's
/// `KeepAlive` can actually see a death and respawn. Launchd only runs its own
/// SIGTERM→SIGKILL escalation when *it* stops the job (`launchctl bootout`);
/// a plain signal sent from outside (what the restart button does — system
/// domain needs root) gets no such bound, so the escalation has to live here.
///
/// Sized above the slowest legitimate clean shutdown on this machine — phoenix
/// measured ~22 s draining — so a healthy exit wins within the window, while a
/// service wedged in graceful teardown is guaranteed
/// to die instead of lingering "not dying, not restarting".
pub const RESTART_GRACE: Duration = Duration::from_secs(30);

/// Whether a PID still exists.
///
/// Same-uid signalling means permission is never the reason kill(2) fails
/// here: `ESRCH` means the process is gone, anything else means it is present.
pub fn process_alive(pid: u32) -> bool {
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(e) => e != libc::ESRCH,
        None => false,
    }
}

/// SIGKILL — the escalation step. Safe because the daemons run as the same
/// uid, which is the very property the restart design relies on.
pub fn force_kill(pid: u32) -> Result<(), String> {
    let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    if rc == 0 {
        return Ok(());
    }
    Err(format!(
        "could not SIGKILL {pid}: {}",
        io::Error::last_os_error()
    ))
}

/// Complete the two-phase stop after `restart` has delivered SIGTERM.
///
/// Polls the old PID for up to `RESTART_GRACE` seconds. If the process has not
/// exited by then it is almost certainly wedged in graceful teardown rather
/// than finishing cleanly, so SIGKILL it — launchd `KeepAlive` then sees the
/// death and respawns. Returns true when an escalation was needed.
///
/// Blocking, so it must run on a blocking/background task (the caller spawns
/// it on `spawn_blocking`), never on the async runtime.
pub fn escalate_if_stuck(pid: u32) -> bool {
    let deadline = std::time::Instant::now() + RESTART_GRACE;
    loop {
        if !process_alive(pid) {
            return false;
        }
        if std::time::Instant::now() >= deadline {
            return force_kill(pid).is_ok();
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etime_covers_every_ps_shape() {
        assert_eq!(parse_etime("05:51"), Some(351));
        assert_eq!(parse_etime("04:30:24"), Some(16_224));
        assert_eq!(parse_etime("2-04:30:24"), Some(189_024));
        assert_eq!(parse_etime("garbage"), None);
    }

    #[test]
    fn launchd_rows_are_parsed_and_pid_zero_means_down() {
        let text = "\
some header = {
	services = {
		    1848      - 	com.example.model-worker
		   76949    -15 	com.example.gateway
		       0      1 	com.example.broken
	}
	disabled services = {
		\"com.example.model-worker\" => enabled
	}
";
        let m = parse_launchd_services(text);
        assert_eq!(m.len(), 3, "the disabled block must not contribute rows");
        assert_eq!(m["com.example.model-worker"].pid, Some(1848));
        assert_eq!(m["com.example.model-worker"].last_exit, None);
        assert_eq!(m["com.example.gateway"].last_exit, Some(-15));
        // PID 0 is launchd's way of saying "registered, not running".
        assert_eq!(m["com.example.broken"].pid, None);
        assert_eq!(m["com.example.broken"].last_exit, Some(1));
    }

    #[test]
    fn tree_usage_reaches_grandchildren() {
        // The shape that matters: launchd's PID is a shell, the memory is two
        // levels down. Summing only the root or its direct children reports
        // ~nothing for a service holding gigabytes.
        let mut table = ProcTable {
            children: HashMap::new(),
            mem_bytes: HashMap::new(),
            cpu: HashMap::new(),
            uptime_sec: HashMap::new(),
        };
        table.children.insert(1848, vec![1858]);
        table.children.insert(1858, vec![1979]);
        for (pid, mem) in [(1848u32, 2_000u64), (1858, 2_000), (1979, 9_000_000)] {
            table.mem_bytes.insert(pid, mem);
            table.cpu.insert(pid, 1.0);
        }
        table.uptime_sec.insert(1848, 3_100);

        let u = tree_usage(&table, 1848);
        assert_eq!(u.mem_bytes, 9_004_000);
        assert_eq!(u.proc_count, 3);
        assert_eq!(u.cpu_percent, 3.0);
        // Uptime is the root's, not the newest child's.
        assert_eq!(u.uptime_sec, 3_100);
    }

    #[test]
    fn process_alive_is_false_for_a_non_existent_pid() {
        // A PID far beyond the kernel's range cannot exist, and kill(pid, 0)
        // must report ESRCH for it. (kill(pid, 0) with pid 0 would probe the
        // caller's own process group, which is not what this means.)
        assert!(!process_alive(u32::MAX / 2));
    }

    #[test]
    fn force_kill_terminates_a_live_child() {
        // End-to-end check that the escalation actually kills: spawn a child,
        // confirm it is alive, SIGKILL it, reap it, and confirm it is gone.
        use std::process::Command;
        let mut child = Command::new("sleep").arg("300").spawn().unwrap();
        let pid = child.id();
        assert!(process_alive(pid));
        force_kill(pid).expect("SIGKILL should succeed on a process we own");
        let status = child.wait().unwrap();
        assert_eq!(status.code(), None, "killed by signal, not a clean exit");
        assert!(!process_alive(pid), "a reaped child must report gone");
    }

    #[test]
    fn force_kill_on_a_non_existent_pid_reports_esrch() {
        // Escalation against an already-dead PID must fail cleanly (ESRCH),
        // which is the normal race between our poll and the process exiting.
        assert!(force_kill(u32::MAX / 2).is_err());
    }

    #[test]
    fn tree_walk_terminates_on_a_cycle() {
        // `ps` is a non-atomic snapshot; a self-referential row is nonsense but
        // must not hang the sampler.
        let mut table = ProcTable {
            children: HashMap::new(),
            mem_bytes: HashMap::new(),
            cpu: HashMap::new(),
            uptime_sec: HashMap::new(),
        };
        table.children.insert(10, vec![11]);
        table.children.insert(11, vec![10]);
        table.mem_bytes.insert(10, 1);
        table.mem_bytes.insert(11, 1);

        let u = tree_usage(&table, 10);
        assert_eq!(u.proc_count, 2);
    }
}
