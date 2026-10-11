//! The `/v1` contract as a document: served by a running daemon, committed at the repository root, and
//! describing every shape the API already has (ticket 40).

use keepster_testkit::Harness;
use serde_json::Value;

#[tokio::test]
async fn a_running_daemon_serves_the_document_under_the_prefix_it_describes() {
    let daemon = Harness::start().await;

    let response = daemon.get("/v1/openapi.json").await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let document: Value = serde_json::from_slice(&response.body).expect("a JSON document");
    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(
        document["info"]["version"], "1",
        "the contract is versioned by its URL prefix, not by the build"
    );
    assert!(
        document["paths"]["/v1/openapi.json"]["get"].is_object(),
        "the document describes the route that serves it"
    );
}

#[tokio::test]
async fn the_document_depends_on_the_build_and_never_on_the_instance() {
    let empty = Harness::start().await;
    let busy = Harness::start().await;
    let project = busy.scratch("project");
    busy.post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    let first = empty.get("/v1/openapi.json").await;
    let second = busy.get("/v1/openapi.json").await;

    assert_eq!(
        first.body, second.body,
        "two daemons of one build serve byte-identical documents, whatever they hold"
    );
    assert_eq!(
        first.body,
        keepster_daemon::openapi_json().as_bytes(),
        "the served document is the generated one"
    );
}

/// The document committed at the repository root, which review reads and the compatibility check
/// compares against the last release.
fn committed_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../openapi.json")
}

/// Any byte of drift between the code and the committed document fails, because a document nobody
/// verifies is believed anyway. `make openapi` sets the variable below to rewrite the file instead.
#[test]
fn the_committed_document_is_the_generated_one() {
    let generated = keepster_daemon::openapi_json();

    if std::env::var_os("KEEPSTER_WRITE_OPENAPI").is_some() {
        std::fs::write(committed_path(), generated).expect("the committed document is writable");
        return;
    }

    let committed = std::fs::read_to_string(committed_path()).unwrap_or_default();
    assert!(
        committed == generated,
        "openapi.json differs from the document this code generates; run `make openapi` and review \
         the diff, which is a change to the `/v1` contract"
    );
}

fn document() -> Value {
    serde_json::from_str(keepster_daemon::openapi_json()).expect("a JSON document")
}

/// Every operation in the document, as (method and path, operation).
fn operations(document: &Value) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for (path, item) in document["paths"].as_object().expect("paths") {
        for (method, operation) in item.as_object().expect("a path item") {
            found.push((
                format!("{} {path}", method.to_uppercase()),
                operation.clone(),
            ));
        }
    }
    found
}

#[test]
fn every_route_the_api_serves_is_described() {
    let mut described: Vec<String> = operations(&document())
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    described.sort();

    assert_eq!(
        described,
        [
            "DELETE /v1/roots/{id}",
            "GET /v1/events",
            "GET /v1/openapi.json",
            "GET /v1/roots",
            "GET /v1/roots/{id}",
            "GET /v1/roots/{id}/config",
            "GET /v1/roots/{id}/explain",
            "GET /v1/roots/{id}/snapshots",
            "GET /v1/roots/{id}/snapshots/{snapshot}/content",
            "GET /v1/roots/{id}/snapshots/{snapshot}/entries",
            "GET /v1/status",
            "GET /v1/version",
            "PATCH /v1/roots/{id}",
            "POST /v1/roots",
            "POST /v1/roots/{id}/reconcile",
            "POST /v1/roots/{id}/snapshots",
        ]
    );
}

#[test]
fn every_success_shape_the_api_already_has_is_a_named_schema() {
    let document = document();
    let schemas = document["components"]["schemas"]
        .as_object()
        .expect("schemas");

    for name in [
        "VersionInfo",
        "Status",
        "DaemonStatus",
        "Root",
        "RootState",
        "RegisterRoot",
        "Snapshot",
        "Change",
        "Timeline",
        "Entries",
        "Entry",
        "EntryKind",
        "Withheld",
        "Explanation",
        "Decision",
        "OnDiskKind",
        "RuleReference",
        "RuleEffect",
        "Origin",
        "ConfigLevel",
        "ConfigWarning",
        "RootConfiguration",
        "Reconciled",
        "ReconcileRequest",
        "ConfigKey",
        "ConfigEntry",
        "Event",
        "Problem",
    ] {
        assert!(schemas.contains_key(name), "`{name}` is described");
    }

    let timeline = &document["paths"]["/v1/roots/{id}/snapshots"]["get"]["responses"]["200"];
    assert_eq!(
        timeline["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Timeline"
    );
    let created = &document["paths"]["/v1/roots/{id}/snapshots"]["post"]["responses"]["201"];
    assert_eq!(
        created["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Snapshot"
    );
}

#[test]
fn every_operation_documents_its_failures_as_problems() {
    for (name, operation) in operations(&document()) {
        if name == "GET /v1/openapi.json" || name == "GET /v1/version" {
            // Neither can fail with a problem: both answer from the build alone.
            continue;
        }
        if name == "GET /v1/events" {
            // Answered from memory alone: an identity it does not know is answered with `resync`, which
            // is an event, not a failure.
            continue;
        }
        let responses = operation["responses"].as_object().expect("responses");
        let failures: Vec<_> = responses
            .iter()
            .filter(|(status, _)| status.starts_with('4') || status.starts_with('5'))
            .collect();

        assert!(!failures.is_empty(), "{name} documents how it fails");
        for (status, response) in failures {
            if name.ends_with("/content") && status == "416" {
                // A range outside the content is answered as RFC 9110 prescribes, with no body.
                continue;
            }
            assert_eq!(
                response["content"]["application/problem+json"]["schema"]["$ref"],
                "#/components/schemas/Problem",
                "{name} answers {status} with a problem document"
            );
        }
    }
}

#[test]
fn the_problem_code_is_an_open_string_whose_known_values_are_listed() {
    let document = document();
    let code = &document["components"]["schemas"]["Problem"]["properties"]["code"];

    assert_eq!(code["type"], "string");
    assert!(
        code.get("enum").is_none(),
        "a closed enum would promise that no other code ever appears (ADR 0005)"
    );
    let description = code["description"].as_str().expect("a description");
    for known in [
        "root_not_found",
        "root_overlaps",
        "root_unavailable",
        "snapshot_not_found",
        "path_not_found",
        "path_invalid",
        "not_a_file",
        "not_a_directory",
        "limit_invalid",
        "cursor_invalid",
        "moment_invalid",
        "depth_invalid",
        "registry_failed",
        "capture_failed",
        "clock_unusable",
        "history_unreadable",
        "body_invalid",
        "purge_invalid",
    ] {
        assert!(
            description.contains(&format!("`{known}`")),
            "`{known}` is listed among the known codes"
        );
    }
    assert!(
        description.contains("unknown"),
        "the description tells a client what to do with a code it does not know"
    );
}

#[test]
fn reading_content_documents_ranges_and_revalidation() {
    let document = document();
    let responses =
        &document["paths"]["/v1/roots/{id}/snapshots/{snapshot}/content"]["get"]["responses"];

    for status in ["200", "206", "304", "416"] {
        assert!(responses[status].is_object(), "content documents {status}");
    }
    assert!(responses["200"]["content"]["application/octet-stream"].is_object());
    assert!(responses["200"]["headers"]["ETag"].is_object());
}

#[test]
fn every_operation_has_its_own_stable_name() {
    let mut names: Vec<String> = operations(&document())
        .into_iter()
        .map(|(_, operation)| {
            operation["operationId"]
                .as_str()
                .expect("an operationId")
                .to_owned()
        })
        .collect();
    names.sort();

    // Literal, because a generated client turns each name into a method: renaming a handler must not
    // rename a method in somebody else's code.
    assert_eq!(
        names,
        [
            "explain_path",
            "get_openapi_document",
            "get_root",
            "get_root_config",
            "get_status",
            "get_version",
            "list_entries",
            "list_roots",
            "list_snapshots",
            "move_root",
            "read_content",
            "reconcile_root",
            "register_root",
            "stream_events",
            "take_snapshot",
            "unwatch_root",
        ]
    );
}

#[test]
fn the_event_stream_is_described_with_its_events_and_its_resume_header() {
    let document = document();
    let stream = &document["paths"]["/v1/events"]["get"];

    assert_eq!(
        stream["responses"]["200"]["content"]["text/event-stream"]["schema"]["$ref"],
        "#/components/schemas/Event"
    );
    let parameters = stream["parameters"].as_array().expect("parameters");
    assert!(
        parameters
            .iter()
            .any(|parameter| parameter["name"] == "Last-Event-ID" && parameter["in"] == "header"),
        "the resume header is declared: {parameters:?}"
    );
    let described = stream["responses"]["200"]["description"]
        .as_str()
        .expect("a description");
    assert!(
        described.contains("1000 events") && described.contains("an hour"),
        "the replay buffer's bound is documented: {described}"
    );

    let mut names: Vec<&str> = document["components"]["schemas"]["Event"]["oneOf"]
        .as_array()
        .expect("one shape per event")
        .iter()
        .map(|shape| {
            shape["properties"]["event"]["enum"][0]
                .as_str()
                .expect("each shape names its event")
        })
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "config.invalid",
            "config.reloaded",
            "resync",
            "root.moved",
            "root.registered",
            "root.state_changed",
            "root.unregistered",
            "snapshot.created",
        ]
    );
}
