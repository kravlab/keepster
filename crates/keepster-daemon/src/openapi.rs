//! The `/v1` contract as a document: generated from the API crate's types and the handlers' own
//! annotations, served by the daemon and committed at the repository root (ticket 40).
//!
//! The document depends on the build and never on the instance. Two daemons of one build serve
//! byte-identical documents, which is what lets it be served before authentication: nothing here may
//! ever be filled from configuration or from what a daemon holds, or the document would stop being a
//! contract and start leaking data.

use std::sync::OnceLock;

use axum::http::header;
use axum::response::IntoResponse;
use utoipa::openapi::{InfoBuilder, OpenApi};

/// The document, as JSON, exactly as it is served and committed.
///
/// Pretty-printed with a trailing newline, so the committed file reads as an ordinary text file and its
/// diffs show a contract change line by line.
pub fn openapi_json() -> &'static str {
    static DOCUMENT: OnceLock<String> = OnceLock::new();

    DOCUMENT.get_or_init(|| {
        let mut json = crate::api()
            .into_openapi()
            .to_pretty_json()
            .expect("an OpenAPI document serialises");
        json.push('\n');
        json
    })
}

/// What the document says about itself.
///
/// `version` is `1` rather than the build's version: the contract is versioned by its URL prefix, and a
/// build number here would churn the committed file on every release and train people to regenerate it
/// without reading the diff. The build is served by `GET /v1/version`.
pub(crate) fn base() -> OpenApi {
    OpenApi::new(
        InfoBuilder::new()
            .title("Keepster")
            .version("1")
            .description(Some(DESCRIPTION))
            .build(),
        utoipa::openapi::Paths::new(),
    )
    .merge_from(
        // No operation's body refers to the events, so they are named here: their shapes are contract
        // all the same, typed today and bound to the stream once the document can say so (ticket 14).
        utoipa::openapi::OpenApiBuilder::new()
            .components(Some(
                utoipa::openapi::ComponentsBuilder::new()
                    .schema_from::<keepster_api::Event>()
                    .build(),
            ))
            .build(),
    )
}

const DESCRIPTION: &str = "\
The local HTTP API of the Keepster daemon.

Everything under `/v1` is additive-only: a field, an endpoint or an error code may be added inside \
`/v1`, and anything else waits for `/v2`. A client must therefore ignore a field it does not know.

Every failure is an RFC 9457 problem document (`application/problem+json`) with a `code` extension. \
`code` is the stable part a client branches on; the prose is not. The set of codes is open: it grows \
inside `/v1`, so a client must treat a code it does not know as its HTTP status.";

/// This document.
#[utoipa::path(
    get,
    path = "/v1/openapi.json",
    operation_id = "get_openapi_document",
    tag = "contract",
    responses(
        (status = 200, description = "This document, as OpenAPI 3.1 JSON.", content_type = "application/json"),
    ),
)]
pub(crate) async fn serve() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], openapi_json())
}
