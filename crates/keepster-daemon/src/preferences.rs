//! The web UI's preferences (ticket 66, ticket 16): `GET` and `PUT /v1/preferences`, on both listeners,
//! since the page itself reads and writes them and holds no more privilege than any loopback client.
//!
//! A file that cannot be read is served as the defaults, with the cause in the log: a broken appearance
//! setting must never keep the UI from opening, and the next `PUT` replaces the file.

use std::sync::{Arc, Mutex, PoisonError};

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use keepster_api::{ErrorCode, Preferences as WirePreferences, Problem};
use keepster_store::{Preferences, PreferencesStore, Theme};

use crate::DaemonState;
use crate::problem::ProblemResponse;

/// The web UI's preferences.
#[utoipa::path(
    get,
    path = "/v1/preferences",
    operation_id = "get_preferences",
    tag = "ui",
    responses(
        (status = 200, description = "The preferences, every key present: the defaults (`system`, `20000`, `false`) until something is chosen, and also while the stored file cannot be read, which the log then says.", body = WirePreferences),
    ),
)]
pub(crate) async fn show(State(state): State<DaemonState>) -> Json<WirePreferences> {
    let store = Arc::clone(&state.preferences);
    let stored = blocking(move || {
        let store = lock(&store);
        store.read().unwrap_or_else(|error| {
            tracing::warn!(error = %error, "the preferences cannot be read; serving the defaults");
            Preferences::default()
        })
    })
    .await
    .unwrap_or_else(|error| {
        tracing::warn!(error = %error, "the preferences could not be read; serving the defaults");
        Preferences::default()
    });

    Json(to_wire(stored))
}

/// Replace the web UI's preferences.
#[utoipa::path(
    put,
    path = "/v1/preferences",
    operation_id = "replace_preferences",
    tag = "ui",
    request_body = WirePreferences,
    responses(
        (status = 200, description = "The preferences now stored, which replace the previous ones whole.", body = WirePreferences),
        (status = 400, description = "A theme other than `system`, `light` or `dark`, or a `syntax_line_limit` outside 1 to 10 000 000 (`preference_invalid`); or a body that is not JSON (`body_invalid`). Nothing was stored.", body = Problem, content_type = "application/problem+json"),
        (status = 415, description = "The body is not declared as `application/json` (`body_invalid`).", body = Problem, content_type = "application/problem+json"),
        (status = 422, description = "The body is JSON of the wrong shape, a key missing or a limit that is not a whole number included (`body_invalid`). Nothing was stored.", body = Problem, content_type = "application/problem+json"),
        (status = 500, description = "The preferences could not be written; the ones stored before are unchanged (`preferences_failed`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn replace(
    State(state): State<DaemonState>,
    body: Result<Json<WirePreferences>, JsonRejection>,
) -> Result<Json<WirePreferences>, ProblemResponse> {
    let Json(request) = body.map_err(|rejection| {
        ProblemResponse::new(
            rejection.status(),
            ErrorCode::BodyInvalid,
            rejection.body_text(),
        )
    })?;
    let chosen = from_wire(&request)?;

    let store = Arc::clone(&state.preferences);
    // Written and synced to disk, which is no work for the thread serving requests.
    blocking(move || lock(&store).write(&chosen))
        .await
        .map_err(failed)?
        .map_err(failed)?;

    Ok(Json(to_wire(chosen)))
}

/// The preferences a request names, or why one of its values is not taken.
fn from_wire(request: &WirePreferences) -> Result<Preferences, ProblemResponse> {
    let invalid = |detail: String| {
        ProblemResponse::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::PreferenceInvalid,
            detail,
        )
    };

    let theme = match request.theme.as_str() {
        "system" => Theme::System,
        "light" => Theme::Light,
        "dark" => Theme::Dark,
        other => {
            return Err(invalid(format!(
                "`{other}` is not a theme; write system, light or dark"
            )));
        }
    };
    let limits = Preferences::SYNTAX_LINE_LIMITS;
    let syntax_line_limit = u32::try_from(request.syntax_line_limit)
        .ok()
        .filter(|limit| limits.contains(limit))
        .ok_or_else(|| {
            invalid(format!(
                "`syntax_line_limit` is {}; it takes {} to {}",
                request.syntax_line_limit,
                limits.start(),
                limits.end()
            ))
        })?;

    Ok(Preferences {
        theme,
        syntax_line_limit,
        wrap_lines: request.wrap_lines,
    })
}

/// The preferences as the wire spells them.
fn to_wire(preferences: Preferences) -> WirePreferences {
    let theme = match preferences.theme {
        Theme::System => "system",
        Theme::Light => "light",
        Theme::Dark => "dark",
    };

    WirePreferences {
        theme: theme.to_owned(),
        syntax_line_limit: i64::from(preferences.syntax_line_limit),
        wrap_lines: preferences.wrap_lines,
    }
}

/// The problem a write that did not happen is reported as.
fn failed(error: impl std::fmt::Display) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::PreferencesFailed,
        error.to_string(),
    )
}

/// Run file work off the threads serving requests; a panic in it is reported as an I/O failure.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(std::io::Error::other)
}

/// The preferences file, held for one read or one write. A writer that panicked left the file whole,
/// since every write replaces it atomically, so a poisoned lock is still a usable one.
fn lock(store: &Mutex<PreferencesStore>) -> std::sync::MutexGuard<'_, PreferencesStore> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}
