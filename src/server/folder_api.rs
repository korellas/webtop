//! REST surface for the Disk drawer's "largest folders" view.
//!
//! Three endpoints, matching the three ways the data can move:
//!   - `GET  /api/folders`        — cached rows, always instant
//!   - `POST /api/folders/verify` — re-measure the visible rows, 3 s budget
//!   - `POST /api/folders/rescan` — kick off a full background walk
//!
//! The drawer calls GET first so it paints immediately, then calls verify and
//! patches in whatever came back fresh. That gives a 0 ms first paint and a
//! live-looking correction a moment later.

use crate::server::control_api::guard;
use crate::server::folder_scan;
use crate::server::AppState;
use crate::storage::folders::FolderRow;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

/// Children returned per level. Five is what the drawer shows; asking for more
/// only costs an index walk.
const DEFAULT_LIMIT: u32 = 5;
const MAX_LIMIT: u32 = 50;

/// Paths accepted per verify call — an upper bound on how much work one
/// request can queue, independent of the time budget.
const MAX_VERIFY_PATHS: usize = 16;

#[derive(Deserialize)]
pub struct FoldersQuery {
    /// Absolute path to list children of. Defaults to the scan root (home).
    pub path: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize)]
pub struct FoldersResponse {
    /// The directory these children belong to.
    pub path: String,
    /// Its own recorded row, when we have one — gives the UI a parent total
    /// without a second request.
    pub total: Option<FolderRow>,
    pub children: Vec<FolderRow>,
    /// Unix ms of the last completed full scan, or null if none has finished.
    pub last_full_scan_at: Option<i64>,
    /// True while a scan is in flight, so the UI can show activity.
    pub scanning: bool,
    /// True when no scan has ever completed — the drawer shows "first scan
    /// pending" rather than an empty list that looks like an error.
    pub never_scanned: bool,
}

#[derive(Deserialize)]
pub struct VerifyRequest {
    pub paths: Vec<String>,
}

#[derive(Serialize)]
pub struct VerifyResponse {
    /// Only the rows that were actually re-measured within the budget.
    pub updated: Vec<FolderRow>,
    /// False when another scan held the lock and nothing was measured.
    pub ran: bool,
}

#[derive(Serialize)]
pub struct RescanResponse {
    /// False when a scan was already running, so the button is a no-op.
    pub started: bool,
}

pub async fn get_folders(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FoldersQuery>,
) -> Result<Json<FoldersResponse>, StatusCode> {
    let path = match params.path {
        Some(p) => PathBuf::from(p),
        None => folder_scan::scan_root(),
    };

    // The path comes from the client, so it must be confined to the tree we
    // actually scan — this endpoint should not become a way to probe whether
    // arbitrary filesystem paths exist.
    if !folder_scan::is_within_root(&path) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let path_str = path.to_string_lossy().into_owned();
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

    let db = &state.db;
    let children = db
        .folder_children(&path_str, limit)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let total = db.folder_entry(&path_str).unwrap_or(None);
    let last_full_scan_at = db.last_full_scan_at();

    Ok(Json(FoldersResponse {
        path: path_str,
        total,
        children,
        last_full_scan_at,
        scanning: state.folder_scan.is_running(),
        never_scanned: last_full_scan_at.is_none(),
    }))
}

pub async fn verify_folders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<VerifyResponse>, StatusCode> {
    guard(&headers).map_err(|(status, _)| status)?;

    let paths: Vec<String> = req
        .paths
        .into_iter()
        .filter(|p| folder_scan::is_within_root(std::path::Path::new(p)))
        .take(MAX_VERIFY_PATHS)
        .collect();

    if paths.is_empty() {
        return Ok(Json(VerifyResponse {
            updated: vec![],
            ran: false,
        }));
    }

    // The walk is blocking and can burn the full budget, so it must not run on
    // a Tokio worker.
    let state2 = Arc::clone(&state);
    let measured = tokio::task::spawn_blocking(move || {
        folder_scan::run_verify(&state2.db, &state2.folder_scan, &paths)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Re-read through the DB so the response carries the same shape (and the
    // freshly written scanned_at) as the GET path.
    let updated: Vec<FolderRow> = measured
        .iter()
        .filter_map(|f| {
            state
                .db
                .folder_entry(&f.path.to_string_lossy())
                .ok()
                .flatten()
        })
        .collect();

    Ok(Json(VerifyResponse {
        ran: !updated.is_empty(),
        updated,
    }))
}

pub async fn rescan_folders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<RescanResponse>, StatusCode> {
    guard(&headers).map_err(|(status, _)| status)?;

    if state.folder_scan.is_running() {
        return Ok(Json(RescanResponse { started: false }));
    }
    folder_scan::spawn_full_scan(Arc::clone(&state.db), Arc::clone(&state.folder_scan));
    Ok(Json(RescanResponse { started: true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::MetricsDb;
    use crate::system_info::SystemInfo;
    use axum::{body::Body, http::Request, routing::post, Router};
    use tower::ServiceExt;

    fn test_state() -> Arc<AppState> {
        AppState::new(
            SystemInfo {
                model: "Test Mac".into(),
                chip: "test".into(),
                p_core_count: 0,
                e_core_count: 0,
                gpu_core_count: 0,
                mem_total: 0,
                disk_total: 0,
                os_version: "test".into(),
                net_link_speed_bytes_sec: 0,
                core_kinds: vec![],
            },
            Arc::new(MetricsDb::open(":memory:").unwrap()),
            std::path::PathBuf::from("/nonexistent/services.json"),
            "/nonexistent/helper".into(),
        )
    }

    #[tokio::test]
    async fn verify_rejects_requests_without_the_control_header() {
        let app = Router::new()
            .route("/verify", post(verify_folders))
            .with_state(test_state());

        let response = app
            .oneshot(
                Request::post("/verify")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"paths":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
}
