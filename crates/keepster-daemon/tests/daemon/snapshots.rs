//! Taking a Snapshot on demand, at the harness seam.

use keepster_testkit::Harness;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn a_snapshot_can_be_taken_on_demand() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.md"), "something worth keeping").expect("a file");

    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = root["id"].as_str().expect("an id").to_owned();

    let taken = daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &serde_json::json!({}))
        .await;

    assert_eq!(
        taken["id"].as_str().expect("an identity").len(),
        26,
        "a Snapshot is identified by a ULID"
    );
    // The Segment is the hour the Snapshot fell in, and the answer has to agree with itself: the
    // Segment name is the first thirteen characters of the moment it reports, in UTC.
    let taken_at: jiff::Timestamp = taken["taken_at"]
        .as_str()
        .expect("a moment")
        .parse()
        .expect("an RFC 3339 moment");
    assert_eq!(
        taken["segment"].as_str().expect("a segment"),
        taken_at
            .to_zoned(jiff::tz::TimeZone::UTC)
            .strftime("%Y-%m-%dT%H")
            .to_string(),
        "the Segment names the hour the Snapshot says it was taken in"
    );
    assert_eq!(
        taken["incomplete"], false,
        "every file was readable, so the Snapshot is whole"
    );
    assert!(
        taken.get("omitted").is_none(),
        "and it lists nothing as omitted"
    );

    // The History Store now exists beside the Root's record, which is where ticket 03 puts it.
    assert!(
        daemon.store_dir(&id).join("history.git/HEAD").is_file(),
        "the Root's History Store was created"
    );
}

#[tokio::test]
async fn snapshots_of_one_root_accumulate_in_one_segment() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.md"), "first").expect("a file");

    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = root["id"].as_str().expect("an id").to_owned();
    let path = format!("/v1/roots/{id}/snapshots");

    let first = daemon.post_json(&path, &serde_json::json!({})).await;
    std::fs::write(project.join("notes.md"), "second").expect("an edit");
    let second = daemon.post_json(&path, &serde_json::json!({})).await;

    assert_ne!(first["id"], second["id"]);
    assert_eq!(
        first["segment"], second["segment"],
        "two Snapshots in the same hour share a Segment"
    );
}

#[tokio::test]
async fn taking_a_snapshot_of_an_unknown_root_is_not_found() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;

    let answer = daemon
        .post(
            "/v1/roots/01ARZ3NDEKTSV4RRFFQ69G5FAV/snapshots",
            &serde_json::json!({}),
        )
        .await;

    assert_eq!(answer.status, 404);
    let problem: serde_json::Value =
        serde_json::from_slice(&answer.body).expect("a problem document");
    assert_eq!(problem["code"], "root_not_found");
}

#[tokio::test]
async fn the_cli_takes_a_snapshot_of_the_root_it_is_standing_in() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let inside = daemon.scratch("project/src");
    std::fs::write(project.join("notes.md"), "something").expect("a file");

    daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    let run = daemon.run_cli_in(&inside, KEEPSTER, &["snapshot"]).await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let printed = String::from_utf8(run.stdout).expect("text");

    // The command has to name the Snapshot it actually took, so the printed identity is matched against
    // the Root's only Snapshot rather than against a word that is in the output unconditionally.
    let identity = printed
        .split_whitespace()
        .find(|word| word.len() == 26 && word.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_else(|| panic!("the printed line names the Snapshot, in:\n{printed}"));
    let second = daemon
        .post_json(
            &format!(
                "/v1/roots/{}/snapshots",
                daemon
                    .get_json("/v1/roots")
                    .await
                    .as_array()
                    .expect("the Roots")[0]["id"]
                    .as_str()
                    .expect("an id")
            ),
            &serde_json::json!({}),
        )
        .await;
    assert_ne!(
        second["id"].as_str().expect("an identity"),
        identity,
        "the identity the CLI printed belongs to the Snapshot it took, not to the next one"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_snapshot_that_could_not_read_a_file_says_so() {
    use std::os::unix::fs::PermissionsExt;

    // Ticket 05: an honest partial Snapshot beats no Snapshot, as long as it never claims to be whole.
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.md"), "readable").expect("a file");
    let locked = project.join("locked.md");
    std::fs::write(&locked, "unreadable").expect("a file");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("its mode");

    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = root["id"].as_str().expect("an id").to_owned();

    let taken = daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &serde_json::json!({}))
        .await;

    assert_eq!(
        taken["incomplete"], true,
        "the Snapshot admits it is not whole: {taken}"
    );
    let omitted = taken["omitted"].as_array().expect("the omitted paths");
    assert_eq!(
        omitted.len(),
        1,
        "and names what it could not read: {taken}"
    );
    assert!(
        omitted[0].as_str().expect("a path").ends_with("locked.md"),
        "by path: {taken}"
    );
}
