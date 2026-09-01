//! HTTP surface for the services panel.
//!
//! Two endpoints: what is declared and how it is doing, and restart.

use crate::server::control_api::{guard, ControlResponse};
use crate::server::AppState;
use crate::services::probe;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize)]
pub struct ServicesResponse {
    pub services: Vec<probe::ServiceStatus>,
    /// Where the manifest was read from, so the panel can name the file when
    /// it has nothing to show.
    pub manifest_path: String,
    /// Populated when the manifest is missing or malformed. The panel shows
    /// this instead of an unexplained empty list — "no services" and "your
    /// manifest has a typo on line 40" look identical otherwise.
    pub error: Option<String>,
}

pub async fn get_services(State(state): State<Arc<AppState>>) -> Json<ServicesResponse> {
    let manifest_path = state.services.path().display().to_string();

    // Probing shells out to `launchctl` and `ps` and opens TCP sockets. On the
    // async runtime that would block a worker for as long as the slowest
    // connect timeout, so it goes to the blocking pool.
    let source = Arc::clone(&state.services);
    let result =
        tokio::task::spawn_blocking(move || source.load().map(|defs| probe::probe_all(&defs)))
            .await;

    match result {
        Ok(Ok(services)) => Json(ServicesResponse {
            services,
            manifest_path,
            error: None,
        }),
        Ok(Err(e)) => Json(ServicesResponse {
            services: Vec::new(),
            manifest_path,
            error: Some(e),
        }),
        Err(e) => Json(ServicesResponse {
            services: Vec::new(),
            manifest_path,
            error: Some(format!("probe task failed: {e}")),
        }),
    }
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    pub range: Option<String>,
}

#[derive(Serialize)]
pub struct RestartResponse {
    pub ok: bool,
    pub message: String,
}

/// Restart one service by signalling its process tree root.
///
/// Only names present in the manifest are accepted, and the PID comes from
/// launchd rather than from the request. That is the whole authorisation
/// story: a caller can ask for a restart of something the operator declared,
/// and cannot use this endpoint to signal an arbitrary PID.
///
/// It is otherwise unauthenticated and reachable from the LAN, which was an
/// explicit decision (spec D10, 2026-07-31) — anyone on the network can bounce
/// a 27 B model server and make it unavailable for minutes. Restricting this
/// to loopback is a peer-address check away if that stops being acceptable.
///
/// A restart is the supervisor's two-phase stop: SIGTERM is delivered
/// immediately, then a background task escalates to SIGKILL if the process is
/// still alive after `RESTART_GRACE`. The HTTP request returns as soon as the
/// SIGTERM is sent so the frontend's restart tracking (which waits for the PID
/// to change) starts immediately; the escalation is fire-and-forget and keeps
/// running even if the browser disconnects.
pub async fn restart_service(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Json<RestartResponse>, (StatusCode, Json<ControlResponse>)> {
    // Same drive-by guard as the other verbs. This endpoint predates them and
    // was left unguarded; a defence that covers four of five mutating routes
    // is not a defence.
    guard(&headers)?;

    let source = Arc::clone(&state.services);
    // `name` is moved into the probe closure below; this own copy is for the
    // escalation task and the response message, which run after that move.
    let display_name = name.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let defs = source.load()?;
        let def = defs
            .iter()
            .find(|d| d.name == name)
            .ok_or_else(|| format!("no service named '{name}' in the manifest"))?;
        let statuses = probe::probe_all(std::slice::from_ref(def));
        let status = statuses
            .first()
            .ok_or_else(|| format!("could not probe '{name}'"))?;
        let pid = status.pid;
        probe::restart(status).map(|()| pid)
    })
    .await
    .map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ControlResponse {
                ok: false,
                message: "restart task panicked".to_string(),
            }),
        )
    })?;

    match outcome {
        Ok(pid) => {
            // Two-phase stop: on a background task, SIGKILL the old PID if it
            // is still alive at the end of the grace window. Blocking (it
            // sleeps), so spawn_blocking rather than on the async runtime.
            if let Some(pid) = pid {
                let name_for_task = display_name.clone();
                tokio::task::spawn_blocking(move || {
                    if probe::escalate_if_stuck(pid) {
                        tracing::warn!(
                            "service {name_for_task}: escalated to SIGKILL after restart"
                        );
                    }
                });
            }
            let message =
                format!("sent SIGTERM to {display_name} (pid {pid:?}); launchd will restart it");
            Ok(Json(RestartResponse { ok: true, message }))
        }
        Err(message) => Ok(Json(RestartResponse { ok: false, message })),
    }
}
