pub mod collector;
pub mod config;
pub mod launchd;
pub mod logging;
pub mod server;
pub mod services;
pub mod storage;
pub mod sync;
pub mod system_info;

use crate::collector::metrics::MetricsCollector;
use crate::config::{Command, Config};
use crate::server::AppState;
use crate::storage::db::MetricsDb;
use axum::routing::get;
use axum::Router;
use clap::Parser;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::compression::CompressionLayer;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::parse();

    // Handle install/uninstall/status subcommands before starting the server.
    if let Some(cmd) = &config.cmd {
        match cmd {
            Command::Install { port } => {
                let port = port.unwrap_or(config.port);
                if let Err(e) = launchd::install(port) {
                    eprintln!("install failed: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Command::Uninstall => {
                if let Err(e) = launchd::uninstall() {
                    eprintln!("uninstall failed: {e}");
                    std::process::exit(1);
                }
                return;
            }
            Command::Status => {
                launchd::status();
                return;
            }
        }
    }

    // Cap the launchd-captured logs before doing anything else — if we are
    // here because of a crash-loop, this is what stops it filling the disk.
    logging::spawn_rotator(config::dirs_home());

    let system_info = system_info::SystemInfo::gather();
    let db_path = config.resolved_db_path();
    let db = match MetricsDb::open(&db_path) {
        Ok(db) => Arc::new(db),
        Err(e) => {
            eprintln!("webtop: cannot open metrics database at {db_path}: {e}");
            eprintln!("        check that the directory exists and is writable.");
            std::process::exit(1);
        }
    };
    let state = AppState::new(
        system_info,
        db.clone(),
        config.resolved_manifest_path(),
        config.control_helper.clone(),
    );

    spawn_collector(state.clone(), db.clone());
    server::folder_scan::spawn_periodic(db, Arc::clone(&state.folder_scan));

    let app = Router::new()
        .route("/ws", get(server::ws::ws_handler))
        .route("/api/system", get(server::api::get_system_info))
        .route("/api/history", get(server::api::get_history))
        .route("/api/network_totals", get(server::api::get_network_totals))
        .route("/api/processes", get(server::api::get_processes))
        .route("/api/disks", get(server::api::get_disks))
        .route(
            "/api/network_interfaces",
            get(server::api::get_network_interfaces),
        )
        .route("/api/gpu_processes", get(server::api::get_gpu_processes))
        .route("/api/energy_history", get(server::api::get_energy_history))
        .route(
            "/api/network_history",
            get(server::api::get_network_history),
        )
        .route("/api/services", get(server::services_api::get_services))
        .route(
            "/api/services/{name}/restart",
            axum::routing::post(server::services_api::restart_service),
        )
        // start / stop / restart / enable / disable, all delegated to the
        // root-owned control helper. Registered after the restart route so the
        // more specific path keeps its dedicated handler.
        .route(
            "/api/services/{name}/control/{verb}",
            axum::routing::post(server::control_api::control_service),
        )
        .route("/api/folders", get(server::folder_api::get_folders))
        .route(
            "/api/folders/verify",
            axum::routing::post(server::folder_api::verify_folders),
        )
        .route(
            "/api/folders/rescan",
            axum::routing::post(server::folder_api::rescan_folders),
        )
        .fallback(server::static_files::static_handler)
        .with_state(state)
        // Everything on the way out is gzipped. The dependency carried the
        // feature but nothing ever applied the layer, so a cold load pulled
        // 960 kB uncompressed: two JS chunks, the CSS, and a history window
        // that is 340 kB of JSON made almost entirely of repeated field names
        // and digits. Measured 2026-08-28: 960 kB -> 248 kB on the wire, and
        // the history window alone 340 kB -> 61 kB.
        //
        // Safe to apply router-wide: tower-http skips responses that carry no
        // body, which is what a `101 Switching Protocols` WebSocket upgrade
        // is.
        .layer(CompressionLayer::new());

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));

    // Bind before announcing — the old order printed "listening" and then
    // panicked, so the logs claimed a start that never happened.
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            eprintln!("webtop: port {} is already in use.", config.port);
            eprintln!("        another webtop is probably running — check with `webtop status`,");
            eprintln!("        or start this one on a different port with --port.");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("webtop: cannot bind {addr}: {e}");
            std::process::exit(1);
        }
    };

    println!("webtop listening on http://{}", addr);

    if let Err(e) = axum::serve(listener, app).await {
        eprintln!("webtop: server stopped: {e}");
        std::process::exit(1);
    }
}

const ACTIVE_COLLECT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);
const IDLE_COLLECT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

fn next_collector_delay(
    has_live_clients: bool,
    collection_elapsed: std::time::Duration,
) -> std::time::Duration {
    let interval = if has_live_clients {
        ACTIVE_COLLECT_INTERVAL
    } else {
        IDLE_COLLECT_INTERVAL
    };
    interval.saturating_sub(collection_elapsed)
}

fn spawn_collector(state: Arc<AppState>, db: Arc<MetricsDb>) {
    // macmon::Sampler holds raw CoreFoundation pointers that are not Send.
    // Run the collector on a dedicated OS thread and forward snapshots to an
    // async task via a channel.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let activity_tx = state.broadcast_tx.clone();

    std::thread::spawn(move || {
        let mut collector = MetricsCollector::with_db(Some(db));
        loop {
            let started = std::time::Instant::now();
            let snapshot = collector.collect();
            if tx.send(snapshot).is_err() {
                break; // receiver dropped — main process is shutting down
            }
            let delay = next_collector_delay(activity_tx.receiver_count() > 0, started.elapsed());
            if !delay.is_zero() {
                std::thread::sleep(delay);
            }
        }
    });

    tokio::spawn(async move {
        // Periodically fold closed rollup buckets, then prune to 7 days.
        let mut ticks: u64 = 0;
        while let Some(snapshot) = rx.recv().await {
            // Persist raw snapshot so history survives restarts.
            state.db.insert_raw(&snapshot).ok();

            ticks += 1;
            if ticks % 60 == 0 {
                // Fold whatever 4-minute buckets have closed. Cheap when there
                // is nothing to do, and this is also the backfill: the first
                // run after an upgrade finds `through` at zero and folds the
                // whole retained window in one pass.
                state.db.roll_up(snapshot.timestamp).ok();

                let cutoff = snapshot.timestamp.saturating_sub(7 * 24 * 3600 * 1000);
                state.db.prune_raw(cutoff).ok();
                // The rollup is a cache of what the raw rows say, so it
                // retains exactly as long as they do.
                state.db.prune_rollup(cutoff).ok();
                // The prune is the largest write of the cycle. Checkpointing
                // right after it is what keeps the WAL sidecar from growing
                // without bound over long uptimes.
                state.db.checkpoint().ok();
            }

            {
                let mut rb = crate::sync::guard(state.ring_buffer.write());
                rb.push(snapshot.clone());
            }
            let _ = state.broadcast_tx.send(snapshot);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn collector_delay_preserves_active_and_idle_cadence() {
        assert_eq!(
            next_collector_delay(true, Duration::from_millis(1_250)),
            Duration::from_millis(750)
        );
        assert_eq!(
            next_collector_delay(false, Duration::from_millis(1_250)),
            Duration::from_millis(3_750)
        );
        assert_eq!(
            next_collector_delay(true, Duration::from_millis(2_500)),
            Duration::ZERO
        );
    }
}
