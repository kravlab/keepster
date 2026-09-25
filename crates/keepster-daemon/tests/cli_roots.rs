//! The Root commands as a person uses them: registering, listing, unwatching and moving, with the Root
//! resolved the way ticket 07 specifies — from the working directory, from an id, or from a prefix.

use keepster_testkit::Harness;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn watch_names_the_root_it_registered_and_roots_lists_it() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("my-project");

    let watched = daemon
        .run_cli(KEEPSTER, &["watch", &project.to_string_lossy()])
        .await;

    assert_eq!(watched.exit_code, Some(0), "stderr: {}", watched.stderr);
    let printed = String::from_utf8(watched.stdout).expect("text");
    // A Root is named the way a person recognises it: the directory's own name plus enough of the
    // identity to tell two apart (ticket 07).
    assert!(printed.contains("my-project"), "no name in:\n{printed}");

    let listed = daemon.run_cli(KEEPSTER, &["roots"]).await;
    let listed = String::from_utf8(listed.stdout).expect("text");
    assert!(listed.contains("my-project"), "no Root in:\n{listed}");
    assert!(
        listed.contains("capture_unavailable"),
        "the listing must say why a Root is not watched yet, in:\n{listed}"
    );
}

#[tokio::test]
async fn status_lists_registered_roots_and_stops_claiming_success() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    let run = daemon.run_cli(KEEPSTER, &["status"]).await;

    // Nothing is captured yet, so a zero exit — "everything is fine" — would be a lie (ticket 05).
    assert_eq!(
        run.exit_code,
        Some(10),
        "a Root that is not watching must make `status` exit non-zero"
    );
    let printed = String::from_utf8(run.stdout).expect("text");
    assert!(printed.contains("project"), "no Root in:\n{printed}");
}

#[tokio::test]
async fn unwatch_from_inside_the_root_finds_it_without_being_told() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    let inside = daemon.scratch("project/src/deep");

    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    let run = daemon.run_cli_in(&inside, KEEPSTER, &["unwatch"]).await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    assert!(
        daemon
            .get_json("/v1/roots")
            .await
            .as_array()
            .expect("an array")
            .is_empty()
    );
    assert!(
        daemon.store_dir(&id).exists(),
        "unwatching keeps the history"
    );
}

#[tokio::test]
async fn unwatch_accepts_an_unambiguous_prefix_of_the_identity() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    let run = daemon
        .run_cli(KEEPSTER, &["unwatch", "--root", &id[..8]])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
}

#[tokio::test]
async fn unwatch_outside_every_root_says_what_to_do_instead() {
    let daemon = Harness::start().await;
    let elsewhere = daemon.scratch("elsewhere");

    let run = daemon.run_cli_in(&elsewhere, KEEPSTER, &["unwatch"]).await;

    assert_eq!(
        run.exit_code,
        Some(4),
        "nothing found is its own exit code, not the catch-all"
    );
    assert!(
        run.stderr.contains("keepster watch"),
        "the error must point at the command that fixes it, in:\n{}",
        run.stderr
    );
}

#[tokio::test]
async fn purging_history_needs_consent_of_its_own() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    let refused = daemon
        .run_cli(KEEPSTER, &["unwatch", "--root", &id, "--purge"])
        .await;

    assert_ne!(
        refused.exit_code,
        Some(0),
        "erasing history must not happen on one flag"
    );
    assert!(
        daemon.store_dir(&id).exists(),
        "the history must still be there"
    );

    let accepted = daemon
        .run_cli(KEEPSTER, &["unwatch", "--root", &id, "--purge", "--yes"])
        .await;

    assert_eq!(accepted.exit_code, Some(0), "stderr: {}", accepted.stderr);
    assert!(
        !daemon.store_dir(&id).exists(),
        "consent given, the history is erased"
    );
}

#[tokio::test]
async fn move_records_the_new_path() {
    let daemon = Harness::start().await;
    let before = daemon.scratch("before");
    let after = daemon.scratch("after");
    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": before }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    let run = daemon
        .run_cli(KEEPSTER, &["move", "--root", &id, &after.to_string_lossy()])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let listed = daemon.get_json("/v1/roots").await;
    assert_eq!(listed[0]["path"], after.to_string_lossy().as_ref());
}
