//! Registering a Watched Root, at the harness seam: a real daemon, a real directory, a real Registry
//! on disk.

use keepster_testkit::Harness;

#[tokio::test]
async fn a_registered_directory_is_listed_and_survives_a_restart() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");

    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    let id = created["id"].as_str().expect("an id").to_owned();
    assert_eq!(
        id.len(),
        26,
        "a Root is identified by a ULID, whose text form is 26 characters: {id}"
    );
    assert_eq!(created["path"], project.to_string_lossy().as_ref());

    // With every Trigger switched off nothing captures it by itself, so claiming `watching` would be
    // exactly the comfortable lie ticket 05 forbids.
    assert_eq!(created["state"], "degraded");
    assert_eq!(created["state_code"], "capture_unavailable");

    let listed = daemon.get_json("/v1/roots").await;
    assert_eq!(listed.as_array().expect("an array of Roots").len(), 1);
    assert_eq!(listed[0]["id"], id);

    // The Registry holds the one fact git could never reproduce — which directory this history is
    // for — so it has to outlive the process that wrote it.
    let daemon = daemon.restart().await;

    let after = daemon.get_json("/v1/roots").await;
    assert_eq!(after[0]["id"], id, "the Registry did not survive a restart");
    assert_eq!(after[0]["path"], project.to_string_lossy().as_ref());
}

#[tokio::test]
async fn a_root_may_neither_sit_inside_another_nor_contain_one() {
    // Nesting would break the walk-up every CLI command rests on — a file in two Roots makes `log`,
    // `restore` and `diff` all ask which one — and would capture every change into two stores.
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let outer = daemon.scratch("outer");
    let inner = daemon.scratch("outer/inner");

    let registered = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": outer }))
        .await;
    let outer_id = registered["id"].as_str().expect("an id");

    let refused = daemon
        .post("/v1/roots", &serde_json::json!({ "path": inner }))
        .await;

    assert_eq!(refused.status, 409, "a nested Root must be refused");
    let problem: serde_json::Value =
        serde_json::from_slice(&refused.body).expect("a problem document");
    assert_eq!(problem["code"], "root_overlaps");
    assert_eq!(
        problem["details"]["root"], outer_id,
        "the refusal must name the Root in the way"
    );

    // The other direction is the one a containment check alone would miss: a directory that would
    // swallow an existing Root. `outer`'s own parent is the harness directory.
    let parent = outer
        .parent()
        .expect("the scratch directory has a parent")
        .to_path_buf();
    let swallowing = daemon
        .post("/v1/roots", &serde_json::json!({ "path": parent }))
        .await;

    assert_eq!(
        swallowing.status, 409,
        "a Root that would contain an existing one must be refused too"
    );
}

#[tokio::test]
async fn a_path_inside_a_root_resolves_to_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let nested = daemon.scratch("project/src/deep");

    let registered = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    // `?path=` filters the collection rather than returning a different shape — the same rule ticket 07
    // applies to the timeline, where filtering by path yields the same list type so a client renders
    // one table with one code path.
    let resolved = daemon
        .get_json(&format!(
            "/v1/roots?path={}",
            keepster_api::encode_path_query(nested.to_string_lossy().as_bytes())
        ))
        .await;

    let matches = resolved.as_array().expect("an array of Roots");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["id"], registered["id"]);
}

#[tokio::test]
async fn a_path_under_no_root_resolves_to_nothing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let elsewhere = daemon.scratch("elsewhere");

    daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    let resolved = daemon
        .get_json(&format!(
            "/v1/roots?path={}",
            keepster_api::encode_path_query(elsewhere.to_string_lossy().as_bytes())
        ))
        .await;

    assert_eq!(
        resolved.as_array().expect("an array of Roots").len(),
        0,
        "a path under no Root is an empty answer, not a failure"
    );
}

#[tokio::test]
async fn deregistering_keeps_the_history_and_does_not_come_back() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");

    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    let removed = daemon.delete(&format!("/v1/roots/{id}")).await;

    assert_eq!(removed.status, 204);
    assert!(
        daemon
            .get_json("/v1/roots")
            .await
            .as_array()
            .expect("an array")
            .is_empty()
    );
    // Unwatching is often temporary, and the history is the insurance the tool exists to provide
    // (ticket 07): erasing it takes an explicit purge.
    assert!(
        daemon.store_dir(&id).exists(),
        "deregistering must keep the History Store"
    );

    let daemon = daemon.restart().await;
    assert!(
        daemon
            .get_json("/v1/roots")
            .await
            .as_array()
            .expect("an array")
            .is_empty(),
        "a deregistered Root must not reappear after a restart"
    );
}

#[tokio::test]
async fn purging_erases_the_history_it_was_asked_to_erase() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");

    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    let removed = daemon.delete(&format!("/v1/roots/{id}?purge=true")).await;

    assert_eq!(removed.status, 204);
    assert!(
        !daemon.store_dir(&id).exists(),
        "an explicit purge is the one thing that erases a History Store"
    );
    assert!(
        project.exists(),
        "erasing history must never touch the watched directory"
    );
}

/// Watch `project`, holding one file with `content`, and capture it once: a Root with a history to
/// erase. Its identity.
async fn captured(daemon: &Harness, project: &std::path::Path, content: &str) -> String {
    std::fs::write(project.join("notes.md"), content).expect("a file");
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
async fn erasing_the_history_keeps_the_root_watched_and_starts_it_over() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let id = captured(&daemon, &project, "first").await;
    let timeline = format!("/v1/roots/{id}/snapshots");

    let erased = daemon.delete(&format!("/v1/roots/{id}/history")).await;

    assert_eq!(erased.status, 204);
    // Starting the history over is not unwatching (ticket 27): the Root keeps its identity and its
    // place in the Registry, and only what was captured is gone.
    assert_eq!(daemon.get(&format!("/v1/roots/{id}")).await.status, 200);
    assert!(
        !daemon.store_dir(&id).join("history.git").exists(),
        "the History Store is erased"
    );
    assert_eq!(
        daemon.get_json(&timeline).await["snapshots"],
        serde_json::json!([]),
        "nothing captured before the erasing is listed"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("notes.md")).expect("the file"),
        "first",
        "erasing history must never touch the watched directory"
    );

    let restarted = daemon.post_json(&timeline, &serde_json::json!({})).await;
    assert_eq!(
        daemon.get_json(&timeline).await["snapshots"][0]["id"],
        restarted["id"],
        "the next capture starts the history over"
    );
}

#[tokio::test]
async fn the_capture_after_an_erasing_reads_every_file_again() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let id = captured(&daemon, &project, "kept across the erasing").await;
    let timeline = format!("/v1/roots/{id}/snapshots");

    daemon.delete(&format!("/v1/roots/{id}/history")).await;
    let restarted = daemon.post_json(&timeline, &serde_json::json!({})).await;

    // The file did not change, so a stat cache that outlived the store would vouch for content the new
    // store never received (ADR 0014): the capture has to hash it again.
    assert_eq!(restarted["added_paths"], 1, "{restarted}");
    let content = daemon
        .get(&format!(
            "/v1/roots/{id}/snapshots/{}/content?path=notes.md",
            restarted["id"].as_str().expect("an identity")
        ))
        .await;
    assert_eq!(content.status, 200);
    assert_eq!(content.body, b"kept across the erasing");
}

#[tokio::test]
async fn erasing_the_history_of_a_root_no_longer_watched_erases_it_whole() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let id = captured(&daemon, &daemon.scratch("project"), "first").await;
    daemon.delete(&format!("/v1/roots/{id}")).await;

    let erased = daemon.delete(&format!("/v1/roots/{id}/history")).await;

    assert_eq!(erased.status, 204);
    // Its history was all a Root no longer watched still had, so nothing of it is left behind.
    assert!(!daemon.store_dir(&id).exists());
}

#[tokio::test]
async fn erasing_the_history_of_a_root_never_registered_is_not_found() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;

    let answer = daemon
        .delete("/v1/roots/01ARZ3NDEKTSV4RRFFQ69G5FAV/history")
        .await;

    assert_eq!(answer.status, 404);
    let problem: serde_json::Value =
        serde_json::from_slice(&answer.body).expect("a problem document");
    assert_eq!(problem["code"], "root_not_found");
}

#[tokio::test]
async fn roots_no_longer_watched_are_listed_with_where_they_were() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let kept = daemon.scratch("kept");
    let watched = daemon.scratch("watched");
    let kept_id = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": kept }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": watched }))
        .await;
    daemon.delete(&format!("/v1/roots/{kept_id}")).await;

    let listed = daemon.get_json("/v1/unwatched-roots").await;

    // Its history is kept after an unwatch, and its identity is all that reaches it (ticket 53): the
    // listing is how a client learns that identity, to erase that history (ticket 68).
    assert_eq!(
        listed,
        serde_json::json!([{ "id": kept_id, "path": kept.to_string_lossy() }]),
        "only the Root no longer watched is listed"
    );

    daemon.delete(&format!("/v1/roots/{kept_id}/history")).await;
    assert_eq!(
        daemon.get_json("/v1/unwatched-roots").await,
        serde_json::json!([]),
        "a Root whose history is erased is gone from the listing"
    );
}

#[tokio::test]
async fn deregistering_something_that_was_never_registered_is_not_found() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;

    let answer = daemon.delete("/v1/roots/01ARZ3NDEKTSV4RRFFQ69G5FAV").await;

    assert_eq!(answer.status, 404);
    let problem: serde_json::Value =
        serde_json::from_slice(&answer.body).expect("a problem document");
    assert_eq!(problem["code"], "root_not_found");
}

#[tokio::test]
async fn moving_a_root_changes_its_path_and_keeps_its_identity() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let before = daemon.scratch("before");
    let after = daemon.scratch("after");

    let created = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": before }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    let moved = daemon
        .patch(
            &format!("/v1/roots/{id}"),
            &serde_json::json!({ "path": after }),
        )
        .await;

    assert_eq!(moved.status, 200);
    let moved: serde_json::Value = serde_json::from_slice(&moved.body).expect("the Root");
    assert_eq!(moved["id"], id, "a move costs the history nothing");
    assert_eq!(moved["path"], after.to_string_lossy().as_ref());

    let daemon = daemon.restart().await;
    let listed = daemon.get_json("/v1/roots").await;
    assert_eq!(listed[0]["path"], after.to_string_lossy().as_ref());
}

#[tokio::test]
async fn moving_a_root_onto_an_overlap_is_refused() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let one = daemon.scratch("one");
    let two = daemon.scratch("two");
    let inside_two = daemon.scratch("two/inside");

    let first = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": one }))
        .await;
    daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": two }))
        .await;

    let refused = daemon
        .patch(
            &format!("/v1/roots/{}", first["id"].as_str().expect("an id")),
            &serde_json::json!({ "path": inside_two }),
        )
        .await;

    assert_eq!(
        refused.status, 409,
        "the no-overlap rule holds for a move too, or it is bypassed in two steps"
    );
}

/// The content type and the `code` of a refusal, which every endpoint answers as a problem document.
fn refusal(response: &keepster_cli::Response) -> (String, String) {
    let content_type = response
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let code = serde_json::from_slice::<serde_json::Value>(&response.body)
        .ok()
        .and_then(|problem| problem["code"].as_str().map(str::to_owned))
        .unwrap_or_default();

    (content_type, code)
}

#[tokio::test]
async fn a_body_that_is_not_the_one_asked_for_is_refused_as_a_problem() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;

    let response = daemon
        .post("/v1/roots", &serde_json::json!({ "directory": "/tmp" }))
        .await;

    assert!(response.status.is_client_error(), "{}", response.status);
    assert_eq!(
        refusal(&response),
        (
            "application/problem+json".to_owned(),
            "body_invalid".to_owned()
        ),
        "a refusal by the framework is still the one shape every endpoint uses"
    );
}

#[tokio::test]
async fn a_purge_that_is_not_a_boolean_is_refused_as_a_problem() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = root["id"].as_str().expect("an id");

    let response = daemon.delete(&format!("/v1/roots/{id}?purge=maybe")).await;

    assert_eq!(response.status, 400);
    assert_eq!(
        refusal(&response),
        (
            "application/problem+json".to_owned(),
            "purge_invalid".to_owned()
        )
    );
    daemon.get_json(&format!("/v1/roots/{id}")).await;
}
