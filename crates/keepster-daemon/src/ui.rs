//! The browser's way in, asked for over the control channel alone (ticket 60): a single-use bootstrap
//! code for `keepster ui`, and revoking every session with `keepster ui token rotate`.
//!
//! Neither is served on the loopback listener: a code is what that listener exchanges for a session, so
//! minting one belongs to whoever the OS lets reach the socket — the machine's owner.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use keepster_api::{ErrorCode, Problem, UiBootstrap};

use crate::DaemonState;
use crate::problem::ProblemResponse;

/// A single-use code for opening a browser session, inside the URL to open.
#[utoipa::path(
    post,
    path = "/v1/ui/bootstrap",
    operation_id = "create_ui_bootstrap",
    tag = "ui",
    responses(
        (status = 200, description = "The URL to open. Its code is exchanged once, for an `HttpOnly`, `SameSite=Strict` cookie, and redirected away from. Served on the control channel alone.", body = UiBootstrap),
        (status = 409, description = "The daemon has no loopback listener: `ui.listener` is off, or it could not bind (`listener_unavailable`).", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "No code could be minted.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn bootstrap(
    State(state): State<DaemonState>,
) -> Result<Json<UiBootstrap>, ProblemResponse> {
    let gate = state.listener.gate()?;
    let url = gate.bootstrap().map_err(failed)?;

    Ok(Json(UiBootstrap { url }))
}

/// Replace the token, revoking every browser session and every code not yet exchanged.
#[utoipa::path(
    post,
    path = "/v1/ui/token/rotate",
    operation_id = "rotate_ui_token",
    tag = "ui",
    responses(
        (status = 204, description = "A new token is in force and in its file; every session the old one opened is refused from now on. A restart rotates it too. Served on the control channel alone."),
        (status = 409, description = "The daemon has no loopback listener, so there is no token (`listener_unavailable`).", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The new token could not be written; the old one stays in force.", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn rotate(
    State(state): State<DaemonState>,
) -> Result<StatusCode, ProblemResponse> {
    let gate = std::sync::Arc::clone(state.listener.gate()?);
    // Written and synced to disk, which is no work for the thread serving requests.
    tokio::task::spawn_blocking(move || gate.rotate())
        .await
        .map_err(std::io::Error::other)
        .and_then(|rotated| rotated)
        .map_err(failed)?;

    Ok(StatusCode::NO_CONTENT)
}

fn failed(error: std::io::Error) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::TokenFailed,
        format!("the token could not be minted or written: {error}"),
    )
}
