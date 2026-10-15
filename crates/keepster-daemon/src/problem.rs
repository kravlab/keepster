//! Turning a failure into the one response shape every endpoint uses.

use std::collections::BTreeMap;

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use keepster_api::{ErrorCode, Problem};

/// A failure on its way to a client, as RFC 9457 Problem Details.
///
/// The media type matters as much as the body: it is what tells a client's library that this is a
/// problem document rather than the resource it asked for.
#[derive(Debug, Clone)]
pub struct ProblemResponse {
    status: StatusCode,
    /// Boxed: every handler returns this in its error arm, and the document is far larger than the
    /// success values beside it.
    problem: Box<Problem>,
}

impl ProblemResponse {
    /// Build a problem response.
    ///
    /// `code` is the stable part of the contract and `detail` is not, so the two are given separately
    /// rather than assembled from one string.
    pub fn new(status: StatusCode, code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            status,
            problem: Box::new(Problem {
                title: status.canonical_reason().unwrap_or("Error").to_owned(),
                status: status.as_u16(),
                detail: detail.into(),
                code: code.as_str().to_owned(),
                details: BTreeMap::new(),
                paths: Vec::new(),
            }),
        }
    }

    /// The problem document itself, for an answer that reports a problem without failing with it.
    pub(crate) fn into_problem(self) -> Problem {
        *self.problem
    }

    /// Add one machine-readable particular, so a client composes its own message instead of parsing
    /// ours.
    pub fn with_detail(mut self, key: &str, value: impl Into<String>) -> Self {
        self.problem.details.insert(key.to_owned(), value.into());
        self
    }

    /// Name the Root-relative path the problem is about: `path` holds its text, and `path_bytes` its
    /// exact bytes when the text cannot carry them (ticket 74).
    pub(crate) fn with_path(self, bytes: &[u8]) -> Self {
        let named = self.with_detail("path", keepster_api::path_text(bytes));
        match keepster_api::path_bytes(bytes) {
            Some(exact) => named.with_detail("path_bytes", exact),
            None => named,
        }
    }

    /// Say, for a Restore refused before it wrote anything, what every path of it needs.
    pub(crate) fn with_paths(mut self, paths: Vec<keepster_api::RestoredPath>) -> Self {
        self.problem.paths = paths;
        self
    }
}

impl IntoResponse for ProblemResponse {
    fn into_response(self) -> Response {
        // Plain data with string keys throughout, so there is no failure mode to handle here.
        let body = serde_json::to_vec(&self.problem).expect("a problem document serialises");

        (
            self.status,
            [(header::CONTENT_TYPE, "application/problem+json")],
            body,
        )
            .into_response()
    }
}

/// A configuration file that is invalid, as the one response shape every endpoint uses.
pub(crate) fn invalid_file(error: &keepster_app::config::ConfigError) -> ProblemResponse {
    ProblemResponse::new(
        StatusCode::CONFLICT,
        ErrorCode::ConfigInvalid,
        error.to_string(),
    )
    .with_detail("path", error.path.to_string_lossy())
}

/// A Root whose configuration has no version to apply: some file it depends on never was valid, so it
/// is not captured at all rather than captured by the defaults (ticket 04). The first invalid file is
/// the one named; fixing it either resolves the Root or names the next.
pub(crate) fn config_problem(errors: &[keepster_app::config::ConfigError]) -> ProblemResponse {
    match errors.first() {
        Some(error) => invalid_file(error),
        None => ProblemResponse::new(
            StatusCode::CONFLICT,
            ErrorCode::ConfigInvalid,
            "the configuration has no valid version to apply",
        ),
    }
}
