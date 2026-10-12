//! The version resource, exercised at the harness seam: a real daemon over a real socket.

use keepster_testkit::Harness;

#[tokio::test]
async fn version_reports_the_build_the_api_and_the_full_hash() {
    let daemon = Harness::start().await;

    let body = daemon.get_json("/v1/version").await;

    assert_eq!(body["api_version"], "v1");
    assert_eq!(body["build_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["build_hash"], expected_build_hash());
}

#[tokio::test]
async fn version_reports_the_four_store_format_numbers_apart_from_the_build() {
    // ADR 0010: four numbers with disjoint scopes, none implied by the product version.
    let daemon = Harness::start().await;

    let body = daemon.get_json("/v1/version").await;

    assert_eq!(
        body["store_format"],
        serde_json::json!({
            "container": keepster_store::CONTAINER_VERSION,
            "content": keepster_core::ContentFormat::CURRENT.number(),
            "sidecar": keepster_core::SIDECAR_VERSION,
            "index": keepster_store::INDEX_SCHEMA_VERSION,
        }),
        "{body}"
    );
    let status = daemon.get_json("/v1/status").await;
    assert_eq!(
        status["daemon"]["store_format"], body["store_format"],
        "status carries the row beside the build and the API"
    );
    let run = daemon
        .run_cli(env!("CARGO_BIN_EXE_keepster"), &["status"])
        .await;
    let printed = String::from_utf8_lossy(&run.stdout);
    assert!(
        printed.contains(&format!(
            "store format container {}, content {}, sidecar {}, index {}",
            keepster_store::CONTAINER_VERSION,
            keepster_core::ContentFormat::CURRENT,
            keepster_core::SIDECAR_VERSION,
            keepster_store::INDEX_SCHEMA_VERSION,
        )),
        "{printed}"
    );
}

/// What the hash must be, according to git rather than according to the rule the daemon applies.
///
/// A full hash, never abbreviated: an abbreviation's length depends on repository size and clone
/// depth. A build from a source archive has no repository and says `unknown` instead of inventing one.
/// The build script reruns when `HEAD` moves, so the embedded hash belongs to the commit that was
/// checked out when the daemon was compiled.
fn expected_build_hash() -> String {
    let head = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output();

    match head {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _ => "unknown".to_owned(),
    }
}
