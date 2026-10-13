//! The Root commands as a person uses them: registering, listing, unwatching and moving, with the Root
//! resolved the way ticket 07 specifies — from the working directory, from an id, or from a prefix.

use keepster_testkit::Harness;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn watch_names_the_root_it_registered_and_roots_lists_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
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
        "the listing must say why a Root is not captured by itself, in:\n{listed}"
    );
}

#[tokio::test]
async fn status_lists_registered_roots_and_stops_claiming_success() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
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
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
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
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
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
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
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
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
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

/// A watched Root with one Snapshot, and its identity.
async fn captured(daemon: &Harness) -> String {
    captured_at(daemon, "project").await
}

/// A watched Root at the scratch directory `name`, with one Snapshot, and its identity.
async fn captured_at(daemon: &Harness, name: &str) -> String {
    let project = daemon.scratch(name);
    std::fs::write(project.join("notes.md"), "worth keeping").expect("a file");
    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();
    daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &serde_json::json!({}))
        .await;

    id
}

#[tokio::test]
async fn purge_needs_consent_and_keeps_the_root_watched() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let id = captured(&daemon).await;
    let history = daemon.store_dir(&id).join("history.git");

    let refused = daemon.run_cli(KEEPSTER, &["purge", "--root", &id]).await;

    assert_eq!(refused.exit_code, Some(2), "stderr: {}", refused.stderr);
    assert!(
        refused.stderr.contains("--yes"),
        "the refusal says how to consent, in:\n{}",
        refused.stderr
    );
    assert!(
        history.exists(),
        "erasing history must not happen on one word"
    );

    let accepted = daemon
        .run_cli(KEEPSTER, &["purge", "--root", &id, "--yes"])
        .await;

    assert_eq!(accepted.exit_code, Some(0), "stderr: {}", accepted.stderr);
    assert!(!history.exists(), "consent given, the history is erased");
    assert_eq!(
        daemon.get(&format!("/v1/roots/{id}")).await.status,
        200,
        "the Root stays watched, to start its history over"
    );
}

#[tokio::test]
async fn purge_reaches_a_root_no_longer_watched_by_its_identity() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let id = captured(&daemon).await;
    daemon.delete(&format!("/v1/roots/{id}")).await;

    let run = daemon
        .run_cli(KEEPSTER, &["purge", "--root", &id, "--yes"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    assert!(!daemon.store_dir(&id).exists());
}

#[tokio::test]
async fn unwatch_purge_reaches_a_root_no_longer_watched_by_its_identity() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let id = captured(&daemon).await;
    daemon.delete(&format!("/v1/roots/{id}")).await;

    let run = daemon
        .run_cli(KEEPSTER, &["unwatch", "--root", &id, "--purge", "--yes"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    assert!(!daemon.store_dir(&id).exists());
}

/// Two watched Roots and one no longer watched, each with history: what the all-Roots forms act on.
async fn three_histories(daemon: &Harness) -> (String, String, String) {
    let one = captured_at(daemon, "one").await;
    let two = captured_at(daemon, "two").await;
    let former = captured_at(daemon, "former").await;
    daemon.delete(&format!("/v1/roots/{former}")).await;

    (one, two, former)
}

#[tokio::test]
async fn purge_all_erases_every_history_and_keeps_every_root_watched() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (one, two, former) = three_histories(&daemon).await;

    let refused = daemon.run_cli(KEEPSTER, &["purge", "--all"]).await;

    assert_eq!(refused.exit_code, Some(2), "stderr: {}", refused.stderr);
    for id in [&one, &two] {
        assert!(daemon.store_dir(id).join("history.git").exists());
    }
    assert!(daemon.store_dir(&former).exists());

    let accepted = daemon.run_cli(KEEPSTER, &["purge", "--all", "--yes"]).await;

    assert_eq!(accepted.exit_code, Some(0), "stderr: {}", accepted.stderr);
    for id in [&one, &two] {
        assert!(!daemon.store_dir(id).join("history.git").exists());
        assert_eq!(daemon.get(&format!("/v1/roots/{id}")).await.status, 200);
    }
    // A Root no longer watched is reached too, as `uninstall --history` reaches it.
    assert!(!daemon.store_dir(&former).exists());
}

#[tokio::test]
async fn purge_takes_either_one_root_or_all_of_them() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let id = captured(&daemon).await;

    let refused = daemon
        .run_cli(KEEPSTER, &["purge", "--all", "--root", &id, "--yes"])
        .await;

    assert_eq!(refused.exit_code, Some(2), "stderr: {}", refused.stderr);
    assert!(daemon.store_dir(&id).join("history.git").exists());
}

#[tokio::test]
async fn erasing_every_root_without_consent_is_refused_before_the_daemon_is_asked() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    // Nothing listens here: a refusal that needed the daemon would end as "not running" instead.
    let unused = daemon.scratch("nobody-listens").join("daemon.sock");

    for args in [
        &["purge", "--all"][..],
        &["unwatch", "--all", "--purge"][..],
    ] {
        let run = daemon
            .run_cli_with_control_path(KEEPSTER, &unused, args)
            .await;

        assert_eq!(run.exit_code, Some(2), "{args:?}: {}", run.stderr);
        assert!(
            run.stderr
                .contains("nothing was changed. Pass --yes as well to confirm"),
            "{args:?}: {}",
            run.stderr
        );
    }
}

#[tokio::test]
async fn the_help_says_that_a_root_no_longer_watched_is_erased_whole() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;

    // Its history is all such a Root still has, so erasing its history leaves no record behind either.
    for args in [&["purge", "--help"][..], &["uninstall", "--help"][..]] {
        let run = daemon.run_cli(KEEPSTER, args).await;

        assert_eq!(run.exit_code, Some(0), "{args:?}: {}", run.stderr);
        let help = String::from_utf8(run.stdout).expect("text");
        assert!(
            help.contains("no longer watched") && help.contains("erased whole"),
            "{args:?}:\n{help}"
        );
    }
}

#[tokio::test]
async fn unwatch_all_stops_watching_every_root_and_keeps_every_history() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (one, two, former) = three_histories(&daemon).await;

    let run = daemon.run_cli(KEEPSTER, &["unwatch", "--all"]).await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    assert_eq!(daemon.get_json("/v1/roots").await, serde_json::json!([]));
    for id in [&one, &two, &former] {
        assert!(
            daemon.store_dir(id).join("history.git").exists(),
            "unwatching keeps the history"
        );
    }
}

#[tokio::test]
async fn unwatch_all_purge_needs_consent_and_erases_every_root() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (one, two, former) = three_histories(&daemon).await;

    let refused = daemon
        .run_cli(KEEPSTER, &["unwatch", "--all", "--purge"])
        .await;

    assert_eq!(refused.exit_code, Some(2), "stderr: {}", refused.stderr);
    assert_eq!(
        daemon
            .get_json("/v1/roots")
            .await
            .as_array()
            .expect("an array")
            .len(),
        2,
        "nothing is unwatched without consent"
    );

    let accepted = daemon
        .run_cli(KEEPSTER, &["unwatch", "--all", "--purge", "--yes"])
        .await;

    assert_eq!(accepted.exit_code, Some(0), "stderr: {}", accepted.stderr);
    assert_eq!(daemon.get_json("/v1/roots").await, serde_json::json!([]));
    for id in [&one, &two, &former] {
        assert!(!daemon.store_dir(id).exists());
    }
}

#[tokio::test]
async fn move_records_the_new_path() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
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
