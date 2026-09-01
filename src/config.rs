use clap::{Parser, Subcommand};

#[derive(Parser, Debug, Clone)]
#[command(name = "webtop", about = "macOS system monitor web dashboard")]
pub struct Config {
    #[arg(long, default_value = "7890")]
    pub port: u16,

    #[arg(long, default_value = "~/.webtop/metrics.db")]
    pub db_path: String,

    #[arg(long, default_value = "120")]
    pub electricity_rate: u32,

    /// Manifest describing the custom services to watch. webtop knows nothing
    /// about any particular stack — it reads the merged JSON manifest the
    /// stack's service manager (macosctl) writes on every `apply`, and the
    /// owning stack symlinks it to this default. See `services::manifest` for
    /// the format and the reasoning.
    #[arg(long, default_value = "~/.webtop/services.json")]
    pub services_manifest: String,

    /// Root-owned wrapper used for privileged service control (start, stop,
    /// restart, enable, disable). webtop holds no privilege of its own and
    /// delegates every verb to this helper through NOPASSWD sudo; the helper
    /// owns the authorisation rules. Not hardcoded, because webtop is a
    /// general tool and the owning stack decides where its helper lives.
    #[arg(long, default_value = "/usr/local/sbin/macosctl-helper")]
    pub control_helper: String,

    /// Optional subcommand. When omitted, webtop runs the HTTP server.
    #[command(subcommand)]
    pub cmd: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Install webtop as a LaunchAgent so it starts automatically on login
    /// and restarts on crash. Writes `~/Library/LaunchAgents/com.webtop.plist`.
    Install {
        /// Override the port the LaunchAgent will use. Defaults to the
        /// same `--port` you'd pass for a one-off run.
        #[arg(long)]
        port: Option<u16>,
    },

    /// Remove the LaunchAgent and stop the running instance.
    Uninstall,

    /// Print the resolved LaunchAgent plist path.
    Status,
}

impl Config {
    pub fn resolved_db_path(&self) -> String {
        self.db_path.replace('~', &dirs_home())
    }

    pub fn resolved_manifest_path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(self.services_manifest.replace('~', &dirs_home()))
    }
}

pub fn dirs_home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}
