//! macOS LaunchAgent install/uninstall for webtop.
//!
//! LaunchAgents are the canonical "autostart on login + restart on crash"
//! mechanism on macOS. We generate a minimal plist that points at the
//! current binary path, drop it in `~/Library/LaunchAgents`, and load it
//! via `launchctl`.
//!
//! Design goals:
//! - No external dependencies (just std + std::process::Command).
//! - Idempotent: reinstalling first unloads any running instance, then
//!   loads the fresh plist.
//! - Self-contained: the user just runs `webtop install`; nothing else
//!   needs to happen for login autostart to work.
//!
//! The LaunchAgent runs as the logged-in user, so everything we need
//! (GPU stats, power, process list) works without root.

use crate::config::dirs_home;
use std::path::PathBuf;
use std::process::Command;

const LABEL: &str = "com.webtop";

/// Absolute path of the plist we write.
pub fn plist_path() -> PathBuf {
    let mut p = PathBuf::from(dirs_home());
    p.push("Library/LaunchAgents");
    p.push(format!("{LABEL}.plist"));
    p
}

fn generate_plist(binary: &str, port: u16) -> String {
    let home = dirs_home();
    let log_dir = format!("{home}/.webtop");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{binary}</string>
        <string>--port</string>
        <string>{port}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>

    <!-- Without a throttle, a persistent startup failure (typically the port
         already being bound) makes launchd respawn as fast as we can exit,
         which floods the error log. 10 s between attempts is plenty. -->
    <key>ThrottleInterval</key>
    <integer>10</integer>

    <!-- A background metrics collector should never compete with foreground
         work for CPU or disk bandwidth. -->
    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>

    <!-- Leave room for the SQLite writer to finish its checkpoint on stop. -->
    <key>ExitTimeOut</key>
    <integer>15</integer>

    <key>StandardOutPath</key>
    <string>{log_dir}/webtop.out.log</string>
    <key>StandardErrorPath</key>
    <string>{log_dir}/webtop.err.log</string>
    <key>WorkingDirectory</key>
    <string>{home}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
    </dict>
</dict>
</plist>
"#
    )
}

/// Install the LaunchAgent for the current binary and start it immediately.
pub fn install(port: u16) -> Result<(), String> {
    let binary =
        std::env::current_exe().map_err(|e| format!("could not resolve current exe path: {e}"))?;
    let binary_str = binary
        .to_str()
        .ok_or_else(|| "binary path contains non-UTF-8 bytes".to_string())?;

    let plist = plist_path();
    if let Some(parent) = plist.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    // Ensure log dir exists so LaunchAgent doesn't fail to start.
    let _ = std::fs::create_dir_all(format!("{}/.webtop", dirs_home()));

    let body = generate_plist(binary_str, port);
    std::fs::write(&plist, body)
        .map_err(|e| format!("could not write {}: {e}", plist.display()))?;

    let plist_str = plist
        .to_str()
        .ok_or_else(|| "plist path contains non-UTF-8 bytes".to_string())?;

    // If an older instance is already registered, tear it down first —
    // bootstrapping over a live label fails.
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{}/{LABEL}", gui_domain())])
        .output();

    let out = Command::new("launchctl")
        .args(["bootstrap", &gui_domain(), plist_str])
        .output()
        .map_err(|e| format!("could not invoke launchctl: {e}"))?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !in_gui_session() {
            return Err(format!(
                "launchctl could not reach the GUI session (currently in the \
                 '{}' launchd domain).\n\
                 Run `webtop install` from Terminal.app on the Mac itself — \
                 a LaunchAgent cannot be registered from an SSH or background \
                 session.\nlaunchctl said: {}",
                manager_name(),
                stderr.trim()
            ));
        }
        return Err(format!("launchctl bootstrap failed: {}", stderr.trim()));
    }

    // `bootstrap` registers the job; `enable` clears any lingering disabled
    // override from a previous `bootout -w` / `launchctl disable`.
    let _ = Command::new("launchctl")
        .args(["enable", &format!("{}/{LABEL}", gui_domain())])
        .output();

    println!("Installed LaunchAgent at {}", plist.display());
    println!("webtop is now running on http://localhost:{port}");
    println!("It will start automatically on login and restart if it exits.");
    Ok(())
}

/// The per-user GUI launchd domain, e.g. `gui/501`. LaunchAgents live here;
/// it is only reachable from a logged-in graphical session.
fn gui_domain() -> String {
    // SAFETY: getuid() is always safe — it takes no arguments, touches no
    // memory, and cannot fail.
    let uid = unsafe { libc::getuid() };
    format!("gui/{uid}")
}

/// Name of the launchd session we're running under — `Aqua` for a real login
/// session, `Background` / `StandardIO` for SSH and daemon contexts.
fn manager_name() -> String {
    Command::new("launchctl")
        .arg("managername")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn in_gui_session() -> bool {
    manager_name() == "Aqua"
}

/// Stop and remove the LaunchAgent.
pub fn uninstall() -> Result<(), String> {
    let plist = plist_path();
    if plist.exists() {
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{}/{LABEL}", gui_domain())])
            .output();
        std::fs::remove_file(&plist)
            .map_err(|e| format!("could not remove {}: {e}", plist.display()))?;
        println!("Removed LaunchAgent at {}", plist.display());
    } else {
        println!("No LaunchAgent found at {}", plist.display());
    }
    Ok(())
}

/// Print current LaunchAgent status.
pub fn status() {
    let plist = plist_path();
    println!("Plist path: {}", plist.display());
    println!("Installed: {}", plist.exists());

    let out = Command::new("launchctl").args(["list", LABEL]).output();
    match out {
        Ok(o) if o.status.success() => {
            println!("--- launchctl list {LABEL} ---");
            print!("{}", String::from_utf8_lossy(&o.stdout));
        }
        _ => {
            println!("launchctl reports no running instance for {LABEL}.");
        }
    }
}
