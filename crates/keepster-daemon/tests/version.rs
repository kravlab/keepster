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
