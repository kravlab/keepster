//! The embedded web UI (ticket 64) on the loopback listener: `keepster-ui` decides what each path is
//! answered with, and this turns its answer into a response.
//!
//! Behind the listener's gate like everything else on it, so the SPA needs no route of its own (ticket
//! 60). Every answer carries a Content-Security-Policy that lets the page reach the daemon alone, which
//! is what makes "the page makes no external request" hold in the browser and not only in review.

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use keepster_ui::Answer;

/// What the page may load: only from the daemon. Styles may be inline because CodeMirror inserts its
/// own; images may be `data:` URLs for the same reason.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
     base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

/// The prefix of the API's paths: a path under it that no route took is a missing endpoint, never a
/// page of the app.
const API: &str = "/v1";

/// Answers a request no route took: an embedded file, or the app's page for a path the app routes itself.
///
/// A path under `/v1` is a missing endpoint whatever the method, so it is told apart before the method
/// is: a `POST` to an endpoint this listener does not have is a 404, as it was before the UI was embedded.
pub(crate) async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path();
    if path == API || path.starts_with(&format!("{API}/")) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET, HEAD")],
        )
            .into_response();
    }
    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok());

    let mut response = match keepster_ui::answer(path, if_none_match) {
        Answer::File(file) => (
            [
                (header::CONTENT_TYPE, file.content_type),
                (header::CACHE_CONTROL, file.cache_control.to_owned()),
                (header::ETAG, file.etag),
            ],
            file.body,
        )
            .into_response(),
        Answer::NotModified {
            etag,
            cache_control,
        } => (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, cache_control.to_owned()),
            ],
        )
            .into_response(),
        Answer::NotFound => StatusCode::NOT_FOUND.into_response(),
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );

    response
}
