//! What `status` says about a Root it cannot serve.
//!
//! Ticket 08 settles the shape: a Root that cannot be opened is a *value*, not a startup error, because
//! an all-or-nothing report has nothing to say exactly when the report matters most. So one broken Root
//! must never take the listing — or `status` — down with it.

use keepster_testkit::Harness;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn a_root_whose_directory_vanished_is_unavailable_and_says_how_to_fix_it() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");

    daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    std::fs::remove_dir_all(&project).expect("the directory goes away");

    let listed = daemon.get_json("/v1/roots").await;

    assert_eq!(
        listed[0]["state"], "unavailable",
        "a missing directory is not a capture gap: nothing can be captured at all"
    );
    assert_eq!(listed[0]["state_code"], "root_missing");
    assert_eq!(
        listed[0]["path"],
        project.to_string_lossy().as_ref(),
        "the last known path is what makes the Root findable again"
    );

    // Ticket 07: `status` shows the Unavailable Root with its last known path and prints the exact
    // command that repairs it.
    let run = daemon.run_cli(KEEPSTER, &["status"]).await;
    let printed = String::from_utf8(run.stdout).expect("text");
    assert!(
        printed.contains("keepster move"),
        "an unavailable Root must name the command that fixes it, in:\n{printed}"
    );
}

#[tokio::test]
async fn a_record_from_a_newer_version_makes_one_root_unavailable_and_no_more() {
    let daemon = Harness::start().await;
    let healthy = daemon.scratch("healthy");
    daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": healthy }))
        .await;

    // A store written by a later Keepster: ticket 26 refuses it as narrowly as its level allows — this
    // Root is Unavailable, and everything else keeps working.
    let newer = "01BX5ZZKBKACTAV9WEVGEMMVRZ";
    let directory = daemon.store_dir(newer);
    std::fs::create_dir_all(&directory).expect("the Root's directory");
    std::fs::write(
        directory.join("root.toml"),
        "version = 99\nid = \"01BX5ZZKBKACTAV9WEVGEMMVRZ\"\npath = \"/somewhere/else\"\n",
    )
    .expect("a record from the future");

    let listed = daemon.get_json("/v1/roots").await;

    let roots = listed.as_array().expect("an array");
    assert_eq!(roots.len(), 2, "the healthy Root must still be listed");

    let from_the_future = roots
        .iter()
        .find(|root| root["id"] == newer)
        .expect("the newer Root is reported, not skipped");
    assert_eq!(from_the_future["state"], "unavailable");
    assert_eq!(from_the_future["state_code"], "store_too_new");

    // And `status` still answers, which is the whole point of a per-Root state.
    let run = daemon.run_cli(KEEPSTER, &["status"]).await;
    assert_eq!(
        run.exit_code,
        Some(10),
        "stderr: {}, stdout: {}",
        run.stderr,
        String::from_utf8_lossy(&run.stdout)
    );
}

#[tokio::test]
async fn a_root_resource_is_addressable_by_its_identity() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = created["id"].as_str().expect("an id");

    let fetched = daemon.get_json(&format!("/v1/roots/{id}")).await;

    assert_eq!(fetched["id"], id);
    assert_eq!(fetched["path"], project.to_string_lossy().as_ref());

    let missing = daemon.get("/v1/roots/01ARZ3NDEKTSV4RRFFQ69G5FAV").await;
    assert_eq!(missing.status, 404);
}

#[tokio::test]
async fn resolving_a_relative_path_is_refused_rather_than_guessed_at() {
    // The daemon's own working directory is meaningless to a client, so resolving a relative path
    // against it would answer a question nobody asked.
    let daemon = Harness::start().await;

    let answer = daemon.get("/v1/roots?path=relative/path").await;

    assert_eq!(answer.status, 400);
    let problem: serde_json::Value =
        serde_json::from_slice(&answer.body).expect("a problem document");
    assert_eq!(problem["code"], "path_invalid");
}

#[tokio::test]
async fn exit_codes_follow_the_error_code_not_the_http_status() {
    let daemon = Harness::start().await;
    let outer = daemon.scratch("outer");
    let inner = daemon.scratch("outer/inner");
    daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": outer }))
        .await;

    // A conflict of state, not a usage error: ticket 07 gives it its own code so a script can tell it
    // from "no such Root" and from a dead daemon.
    let overlapping = daemon
        .run_cli(KEEPSTER, &["watch", &inner.to_string_lossy()])
        .await;
    assert_eq!(
        overlapping.exit_code,
        Some(5),
        "stderr: {}",
        overlapping.stderr
    );

    // A path that is not a directory is the user's mistake, which is a usage error.
    let absent = daemon.scratch("gone");
    std::fs::remove_dir_all(&absent).expect("the directory goes away");
    let invalid = daemon
        .run_cli(KEEPSTER, &["watch", &absent.to_string_lossy()])
        .await;
    assert_eq!(invalid.exit_code, Some(2), "stderr: {}", invalid.stderr);
}
