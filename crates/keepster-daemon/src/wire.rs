//! Root-relative paths onto the wire and back (ticket 74).
//!
//! A path leaves as its text — itself when it is UTF-8, its lossy rendering otherwise, which is what a
//! person reads — and a `_bytes` companion carrying its exact bytes where the text cannot. A request's
//! paths come back the other way, the companion winning over its text; one that cannot be read is
//! refused as a malformed path is.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use keepster_api::{ErrorCode, PathResolution};
use keepster_core::RootPath;

use crate::problem::ProblemResponse;
use crate::snapshots::parse_path;

/// A path's text on the wire.
pub(crate) fn text(path: &RootPath) -> String {
    path.to_lossy().into_owned()
}

/// A path's `_bytes` companion on the wire: present only when it is not UTF-8.
pub(crate) fn companion(path: &RootPath) -> Option<String> {
    keepster_api::path_bytes(path.as_bytes())
}

/// Each path's text on the wire.
pub(crate) fn texts<'a>(paths: impl IntoIterator<Item = &'a RootPath>) -> Vec<String> {
    paths.into_iter().map(text).collect()
}

/// A list of paths' `_bytes` companion on the wire: one per path, and none at all when every one is
/// UTF-8.
pub(crate) fn companions<'a>(paths: impl IntoIterator<Item = &'a RootPath>) -> Vec<Option<String>> {
    keepster_api::paths_bytes(paths.into_iter().map(RootPath::as_bytes))
}

/// The paths a request's list and its `_bytes` companion name, each checked as every endpoint checks a
/// path.
pub(crate) fn requested(
    texts: &[String],
    bytes: &[Option<String>],
) -> Result<Vec<RootPath>, ProblemResponse> {
    keepster_api::paths_from_wire(texts, bytes)
        .map_err(unreadable)?
        .iter()
        .map(|path| parse_path(path))
        .collect()
}

/// The answers a request gives, keyed by the path each is for: `resolutions` and `resolutions_bytes`
/// joined, an answer keyed by bytes winning over one keyed by the same path's text.
pub(crate) fn resolutions(
    text: &BTreeMap<String, PathResolution>,
    bytes: &BTreeMap<String, PathResolution>,
) -> Result<BTreeMap<RootPath, PathResolution>, ProblemResponse> {
    keepster_api::resolutions_from_wire(text, bytes)
        .map_err(unreadable)?
        .into_iter()
        .map(|(path, resolution)| Ok((parse_path(&path)?, resolution)))
        .collect()
}

/// A `_bytes` companion that cannot be read, refused as a malformed path is.
fn unreadable(error: keepster_api::InvalidPathBytes) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::BAD_REQUEST,
        ErrorCode::PathInvalid,
        error.to_string(),
    )
}
