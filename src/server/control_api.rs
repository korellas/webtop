//! Service control: start, stop, restart, enable, disable.
//!
//! webtop holds no privilege of its own. Every verb is delegated to a control
//! helper — a small root-owned wrapper reached through NOPASSWD sudo — which
//! validates the verb, resolves the label from a hardcoded prefix, and checks
//! the target against a root-owned inventory before touching launchd. webtop's
//! job is to pass a name through; the authorisation story lives entirely on the
//! other side of that interface.
//!
//! The helper path is configuration, not a constant: webtop is a general tool
//! and must not hardcode one stack's layout (see `--control-helper`).
//!
//! ## Why these endpoints require a header
//!
//! webtop binds `0.0.0.0` without authentication, and exposing control to the
//! LAN is an accepted risk for this deployment. Drive-by requests are not part
//! of that acceptance: without a guard, any page a browser on this network
//! happens to visit could POST here, because a simple cross-origin POST is
//! sent (and its side effect happens) even though the response is unreadable.
//!
//! Requiring a custom header is the whole defence. Custom headers force a CORS
//! preflight, and webtop answers no preflight, so a cross-origin request never
//! reaches the handler. Two conditions keep that true and both are load-bearing:
//!
//!   1. No permissive CORS layer may ever be added to this router.
//!   2. Every mutating endpoint must carry the guard, including restart.
//!
//! Deliberately *not* claimed: this is not authentication and does not identify
//! the caller. Any LAN client can set a header. It stops drive-by browser
//! requests and nothing else — an `Origin`/`Host` reflection check was
//! considered and rejected, because serving an attack page from a host that
//! matches, then rebinding DNS to this machine, defeats exactly that comparison.

use crate::server::AppState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Serialize;
use std::sync::Arc;

/// Required on every mutating request. Value is irrelevant; presence is the
/// signal, because presence is what forces the preflight.
pub const CONTROL_HEADER: &str = "x-svc-control";

/// Verbs the helper accepts. Kept here so an unknown verb is refused before a
/// process is spawned; the helper validates independently and is the authority.
const VERBS: [&str; 5] = ["start", "stop", "restart", "enable", "disable"];

#[derive(Serialize)]
pub struct ControlResponse {
    pub ok: bool,
    pub message: String,
}

/// Rejects requests that a browser could have made cross-origin.
pub fn guard(headers: &HeaderMap) -> Result<(), (StatusCode, Json<ControlResponse>)> {
    if headers.contains_key(CONTROL_HEADER) {
        return Ok(());
    }
    Err((
        StatusCode::FORBIDDEN,
        Json(ControlResponse {
            ok: false,
            message: format!("missing {CONTROL_HEADER} header; control requests must set it"),
        }),
    ))
}

fn is_known_verb(verb: &str) -> bool {
    VERBS.contains(&verb)
}

/// `POST /api/services/{name}/{verb}`
pub async fn control_service(
    State(state): State<Arc<AppState>>,
    Path((name, verb)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<ControlResponse>, (StatusCode, Json<ControlResponse>)> {
    guard(&headers)?;

    if !is_known_verb(&verb) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ControlResponse {
                ok: false,
                message: format!("unknown verb '{verb}'"),
            }),
        ));
    }

    // Only names the operator declared are forwarded. The helper repeats this
    // check against its own root-owned data — this one exists to fail fast and
    // to give the panel a better message, not as the security boundary.
    let source = Arc::clone(&state.services);
    let wanted = name.clone();
    let known = tokio::task::spawn_blocking(move || {
        source
            .load()
            .map(|defs| defs.iter().any(|d| d.name == wanted))
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);

    if !known {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ControlResponse {
                ok: false,
                message: format!("no service named '{name}' in the manifest"),
            }),
        ));
    }

    let helper = state.control_helper.clone();
    let outcome = tokio::task::spawn_blocking(move || run_helper(&helper, &verb, &name))
        .await
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ControlResponse {
                    ok: false,
                    message: "control task panicked".to_string(),
                }),
            )
        })?;

    match outcome {
        Ok(message) => Ok(Json(ControlResponse { ok: true, message })),
        Err(message) => Ok(Json(ControlResponse { ok: false, message })),
    }
}

/// Runs `sudo -n <helper> <verb> <name>`.
///
/// `-n` so a missing sudoers rule fails immediately instead of blocking on a
/// password prompt no one can answer: this runs under launchd with no terminal,
/// and without it the request would hang until the client gave up.
fn run_helper(helper: &str, verb: &str, name: &str) -> Result<String, String> {
    let output = std::process::Command::new("/usr/bin/sudo")
        .arg("-n")
        .arg(helper)
        .arg(verb)
        .arg(name)
        .output()
        .map_err(|e| format!("could not run {helper}: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        return Ok(if stdout.is_empty() { stderr } else { stdout });
    }

    let detail = if stderr.is_empty() { stdout } else { stderr };
    if detail.contains("password is required") {
        return Err(format!(
            "{helper} is not permitted without a password — install the sudoers rule ({detail})"
        ));
    }
    Err(detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn guard_rejects_request_without_header() {
        assert!(guard(&HeaderMap::new()).is_err());
    }

    #[test]
    fn guard_accepts_request_with_header() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTROL_HEADER, HeaderValue::from_static("1"));
        assert!(guard(&headers).is_ok());
    }

    #[test]
    fn only_the_five_verbs_are_known() {
        for verb in ["start", "stop", "restart", "enable", "disable"] {
            assert!(is_known_verb(verb), "{verb} should be accepted");
        }
        for verb in ["delete", "bootstrap", "", "STOP", "stop; rm -rf /"] {
            assert!(!is_known_verb(verb), "{verb} should be refused");
        }
    }
}
