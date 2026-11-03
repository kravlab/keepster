//! A Root registered or moved through a link (ticket 77), at the `/v1` seam: a path through a link is
//! refused until the request says whether the Root follows the link or where it leads, and the Root says
//! which it does. A Root following its link goes where the link is pointed: watched and captured there,
//! Withholding a Root or a History Store the new target holds, and `Unavailable` while it leads inside
//! another Root.
//!
//! Its own module rather than `links.rs`, whose links are the ones inside a Root (ticket 49); these are
//! on the way to one.
#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::path::PathBuf;

use keepster_path::retarget_link;
use keepster_testkit::Harness;
use serde_json::{Value, json};

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

/// A directory `release` and a link `current` to it, in the harness's scratch space, whose base has no
/// link on the way.
fn linked(daemon: &Harness) -> (PathBuf, PathBuf) {
    let real = daemon.scratch("release");
    let link = real.with_file_name("current");
    symlink(&real, &link).expect("a link");

    (link, real)
}

/// The Roots `GET /v1/roots?path=` finds for `path`.
async fn found_for(daemon: &Harness, path: &std::path::Path) -> Value {
    daemon
        .get_json(&format!(
            "/v1/roots?path={}",
            keepster_api::encode_path_query(path.to_string_lossy().as_bytes())
        ))
        .await
}

/// The JSON body of a response.
fn body(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("a JSON body")
}

#[tokio::test]
async fn a_path_through_a_link_without_a_path_mode_asks_for_one_and_registers_nothing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);

    let refused = daemon.post("/v1/roots", &json!({ "path": link })).await;

    assert_eq!(refused.status, 400);
    let problem = body(&refused);
    assert_eq!(problem["code"], "confirmation_required");
    assert_eq!(problem["details"]["path"], link.to_string_lossy().as_ref());
    assert_eq!(
        problem["details"]["real_path"],
        real.to_string_lossy().as_ref()
    );
    assert_eq!(problem["details"]["missing"], "path_mode");
    assert_eq!(
        daemon.get_json("/v1/roots").await,
        json!([]),
        "nothing is registered until the choice is made"
    );
}

#[tokio::test]
async fn a_canonical_root_is_registered_at_the_real_path() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);

    let created = daemon
        .post(
            "/v1/roots",
            &json!({ "path": link, "path_mode": "canonical" }),
        )
        .await;

    assert_eq!(created.status, 201);
    let root = body(&created);
    assert_eq!(root["path"], real.to_string_lossy().as_ref());
    assert_eq!(root["path_mode"], "canonical");
    assert!(
        root.get("real_path").is_none(),
        "the path is the real one already: {root}"
    );
}

#[tokio::test]
async fn a_link_root_keeps_the_link_and_says_where_it_leads() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);

    let created = daemon
        .post("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;

    assert_eq!(created.status, 201);
    let root = body(&created);
    assert_eq!(root["path"], link.to_string_lossy().as_ref());
    assert_eq!(root["path_mode"], "link");
    assert_eq!(root["real_path"], real.to_string_lossy().as_ref());
    // The answer is judged against the Registry, which now holds this Root itself: it never overlaps itself.
    assert_ne!(root["state_code"], "root_overlaps", "{root}");
    let listed = daemon.get_json("/v1/roots").await;
    assert_eq!(listed[0]["path_mode"], "link", "the listing says so too");
    assert_eq!(listed[0]["real_path"], real.to_string_lossy().as_ref());
}

#[tokio::test]
async fn a_path_through_no_link_has_no_path_mode() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");

    let root = daemon
        .post_json(
            "/v1/roots",
            &json!({ "path": project, "path_mode": "link" }),
        )
        .await;

    assert_eq!(root["path"], project.to_string_lossy().as_ref());
    assert!(root.get("path_mode").is_none(), "{root}");
    assert!(root.get("real_path").is_none(), "{root}");
}

#[tokio::test]
async fn a_path_mode_it_does_not_know_is_refused_as_a_body_it_cannot_read() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, _) = linked(&daemon);

    let refused = daemon
        .post("/v1/roots", &json!({ "path": link, "path_mode": "both" }))
        .await;

    assert!(refused.status.is_client_error(), "{}", refused.status);
    assert_eq!(body(&refused)["code"], "body_invalid");
    assert_eq!(daemon.get_json("/v1/roots").await, json!([]));
}

#[tokio::test]
async fn a_move_through_a_link_asks_for_a_path_mode_and_then_obeys_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let (link, real) = linked(&daemon);
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await;
    let address = format!("/v1/roots/{}", created["id"].as_str().expect("an id"));

    let refused = daemon.patch(&address, &json!({ "path": link })).await;

    assert_eq!(refused.status, 400);
    let problem = body(&refused);
    assert_eq!(problem["code"], "confirmation_required");
    assert_eq!(
        problem["details"]["real_path"],
        real.to_string_lossy().as_ref()
    );
    assert_eq!(problem["details"]["missing"], "path_mode");
    assert_eq!(
        daemon.get_json("/v1/roots").await[0]["path"],
        project.to_string_lossy().as_ref(),
        "a refused move leaves the Root where it was"
    );

    let moved = daemon
        .patch(&address, &json!({ "path": link, "path_mode": "link" }))
        .await;

    assert_eq!(moved.status, 200);
    let root = body(&moved);
    assert_eq!(root["path"], link.to_string_lossy().as_ref());
    assert_eq!(root["path_mode"], "link");
    assert_eq!(root["real_path"], real.to_string_lossy().as_ref());
    assert_ne!(root["state_code"], "root_overlaps", "{root}");

    let moved = daemon
        .patch(&address, &json!({ "path": link, "path_mode": "canonical" }))
        .await;

    assert_eq!(moved.status, 200);
    let root = body(&moved);
    assert_eq!(root["path"], real.to_string_lossy().as_ref());
    assert_eq!(root["path_mode"], "canonical");
    assert!(root.get("real_path").is_none(), "{root}");
}

#[tokio::test]
async fn a_root_through_a_link_into_another_roots_real_tree_overlaps_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    std::fs::create_dir(real.join("app")).expect("a directory inside");
    let existing = daemon
        .post_json("/v1/roots", &json!({ "path": real }))
        .await;

    let refused = daemon
        .post(
            "/v1/roots",
            &json!({ "path": link.join("app"), "path_mode": "link" }),
        )
        .await;

    assert_eq!(refused.status, 409);
    let problem = body(&refused);
    assert_eq!(problem["code"], "root_overlaps");
    assert_eq!(problem["details"]["root"], existing["id"]);
    assert_eq!(
        daemon.get_json("/v1/roots").await.as_array().map(Vec::len),
        Some(1),
        "nothing else is registered"
    );
}

#[tokio::test]
async fn a_canonical_root_stays_with_the_real_directory_when_the_link_is_pointed_elsewhere() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    std::fs::write(real.join("notes.md"), "the release").expect("a file");
    let created = daemon
        .post_json(
            "/v1/roots",
            &json!({ "path": link, "path_mode": "canonical" }),
        )
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();
    let elsewhere = daemon.scratch("elsewhere");
    std::fs::write(elsewhere.join("notes.md"), "somewhere else").expect("a file");
    std::fs::write(elsewhere.join("only-elsewhere.md"), "not ours").expect("a file");
    std::fs::remove_file(&link).expect("the old link removed");
    symlink(&elsewhere, &link).expect("the link pointed elsewhere");

    let root = daemon.get_json(&format!("/v1/roots/{id}")).await;
    let snapshot = daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await["id"]
        .as_str()
        .expect("a Snapshot")
        .to_owned();

    assert_eq!(root["path"], real.to_string_lossy().as_ref(), "{root}");
    assert_eq!(root["path_mode"], "canonical", "{root}");
    assert_eq!(root["state"], created["state"], "{root}");
    let notes = daemon
        .get(&format!(
            "/v1/roots/{id}/snapshots/{snapshot}/content?path=notes.md"
        ))
        .await;
    assert_eq!(
        notes.body, b"the release",
        "captured from the real directory"
    );
    let stranger = daemon
        .get(&format!(
            "/v1/roots/{id}/snapshots/{snapshot}/content?path=only-elsewhere.md"
        ))
        .await;
    assert_eq!(stranger.status, 404, "nothing captured from the new target");
}

#[tokio::test]
async fn a_path_inside_where_a_link_root_leads_finds_that_root() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    std::fs::create_dir(real.join("src")).expect("a directory inside");
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;

    let found = found_for(&daemon, &real.join("src")).await;

    assert_eq!(found.as_array().map(Vec::len), Some(1), "{found}");
    assert_eq!(found[0]["id"], created["id"], "{found}");
}

#[tokio::test]
async fn a_path_spelt_through_the_link_finds_the_canonical_root_at_its_target() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    std::fs::create_dir(real.join("src")).expect("a directory inside");
    let created = daemon
        .post_json(
            "/v1/roots",
            &json!({ "path": link, "path_mode": "canonical" }),
        )
        .await;

    let found = found_for(&daemon, &link.join("src")).await;

    assert_eq!(found.as_array().map(Vec::len), Some(1), "{found}");
    assert_eq!(found[0]["id"], created["id"], "{found}");
}

#[tokio::test]
async fn a_path_found_by_where_it_really_is_before_how_it_is_spelt() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let spelt_in = daemon.scratch("spelt-in");
    let really_in = daemon.scratch("really-in");
    std::fs::create_dir(really_in.join("src")).expect("a directory inside");
    // A link inside one Root to another: spelt, the path is inside the first; really, the second.
    symlink(&really_in, spelt_in.join("elsewhere")).expect("a link");
    daemon
        .post_json("/v1/roots", &json!({ "path": spelt_in }))
        .await;
    let really = daemon
        .post_json("/v1/roots", &json!({ "path": really_in }))
        .await;

    let found = found_for(&daemon, &spelt_in.join("elsewhere").join("src")).await;

    assert_eq!(found.as_array().map(Vec::len), Some(1), "{found}");
    assert_eq!(
        found[0]["id"], really["id"],
        "as the CLI finds it, standing there: {found}"
    );
}

#[tokio::test]
async fn a_path_not_made_yet_spelt_through_the_link_finds_the_root_at_its_target() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, _) = linked(&daemon);
    let created = daemon
        .post_json(
            "/v1/roots",
            &json!({ "path": link, "path_mode": "canonical" }),
        )
        .await;
    let missing = link.join("deleted").join("notes.md");
    assert!(!missing.exists(), "the path must not exist for this test");

    let found = found_for(&daemon, &missing).await;

    assert_eq!(found.as_array().map(Vec::len), Some(1), "{found}");
    assert_eq!(found[0]["id"], created["id"], "{found}");
}

#[tokio::test]
async fn the_cli_standing_inside_the_link_finds_the_canonical_root_at_its_target() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    std::fs::create_dir(real.join("src")).expect("a directory inside");
    daemon
        .post_json(
            "/v1/roots",
            &json!({ "path": link, "path_mode": "canonical" }),
        )
        .await;

    let run = daemon
        .run_cli_in(&link.join("src"), KEEPSTER, &["unwatch"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    assert_eq!(
        daemon.get_json("/v1/roots").await,
        json!([]),
        "the Root at the real path was the one unwatched"
    );
}

/// The Root `id` once `accept` says it is as awaited, polled as the reload tick catches up.
async fn root_once(
    daemon: &Harness,
    id: &str,
    what: &str,
    accept: impl Fn(&Value) -> bool,
) -> Value {
    crate::cli_lifecycle::eventually(what, || async {
        let root = daemon.get_json(&format!("/v1/roots/{id}")).await;
        accept(&root).then_some(root)
    })
    .await
}

#[tokio::test]
async fn a_link_root_led_inside_another_root_is_unavailable_until_the_link_is_pointed_back() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    std::fs::write(real.join("notes.md"), "the release").expect("a file");
    let other = daemon.scratch("other");
    std::fs::create_dir(other.join("inner")).expect("a directory inside");
    daemon
        .post_json("/v1/roots", &json!({ "path": other }))
        .await;
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    retarget_link(&link, &other.join("inner"));

    let root = root_once(&daemon, &id, "the overlap reported", |root| {
        root["state_code"] == "root_overlaps"
    })
    .await;
    assert_eq!(root["state"], "unavailable", "{root}");
    let refused = daemon
        .post(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await;
    assert_eq!(refused.status, 409);
    assert_eq!(
        body(&refused)["code"],
        "root_overlaps",
        "{}",
        body(&refused)
    );

    retarget_link(&link, &real);

    let root = root_once(&daemon, &id, "the overlap lifted", |root| {
        root["state_code"] != "root_overlaps"
    })
    .await;
    assert_eq!(root["state"], created["state"], "{root}");
    assert_eq!(root["state_code"], created["state_code"], "{root}");
    let captured = daemon
        .post(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await;
    assert_eq!(captured.status, 201, "{}", body(&captured));
}

#[tokio::test]
async fn a_link_root_whose_link_leads_nowhere_or_is_gone_is_unavailable_as_missing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    // The link left dangling: what it led to is gone.
    std::fs::remove_dir(&real).expect("the target removed");

    let root = daemon.get_json(&format!("/v1/roots/{id}")).await;
    assert_eq!(root["state"], "unavailable", "{root}");
    assert_eq!(root["state_code"], "root_missing", "{root}");

    // The link itself gone too.
    std::fs::remove_file(&link).expect("the link removed");

    let root = daemon.get_json(&format!("/v1/roots/{id}")).await;
    assert_eq!(root["state"], "unavailable", "{root}");
    assert_eq!(root["state_code"], "root_missing", "{root}");
    let refused = daemon
        .post(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await;
    assert_eq!(refused.status, 409, "{}", body(&refused));
}

#[tokio::test]
async fn a_link_root_says_where_its_link_leads_now_even_where_that_is_not_recorded() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, _) = linked(&daemon);
    let other = daemon.scratch("other");
    let inner = other.join("inner");
    std::fs::create_dir(&inner).expect("a directory inside");
    daemon
        .post_json("/v1/roots", &json!({ "path": other }))
        .await;
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    // Led inside another Root, where the link leads is never recorded: the Root is `Unavailable`.
    retarget_link(&link, &inner);

    let root = root_once(&daemon, &id, "the overlap reported", |root| {
        root["state_code"] == "root_overlaps"
    })
    .await;
    assert_eq!(
        root["real_path"],
        inner.to_string_lossy().as_ref(),
        "{root}"
    );
}

#[tokio::test]
async fn a_link_root_does_not_follow_its_volume_to_another_mount_point() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let disk = daemon.scratch("media").join("disk");
    let real = disk.join("release");
    std::fs::create_dir_all(&real).expect("the target");
    let link = disk.join("current");
    symlink(&real, &link).expect("a link");
    daemon.volumes().mount(&disk, Some("uuid:disk"));
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    // The volume mounted somewhere else, with a directory where the link was within it: a Root that
    // follows its link goes where the link leads, never where the volume went.
    daemon.volumes().unmount(&disk);
    let elsewhere = daemon.scratch("run").join("DISK1");
    std::fs::create_dir_all(elsewhere.join("current")).expect("a directory there");
    daemon.volumes().mount(&elsewhere, Some("uuid:disk"));
    daemon
        .post(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await;

    let root = daemon.get_json(&format!("/v1/roots/{id}")).await;
    assert_eq!(root["path"], link.to_string_lossy().as_ref(), "{root}");
    assert_eq!(root["path_mode"], "link", "{root}");
}

#[tokio::test]
async fn two_link_roots_led_to_one_directory_leave_the_first_registered_capturing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (first_link, real) = linked(&daemon);
    std::fs::write(real.join("notes.md"), "the release").expect("a file");
    let next = daemon.scratch("next");
    let second_link = next.with_file_name("second");
    symlink(&next, &second_link).expect("a link");
    let first = daemon
        .post_json(
            "/v1/roots",
            &json!({ "path": first_link, "path_mode": "link" }),
        )
        .await;
    let first_id = first["id"].as_str().expect("an id").to_owned();
    let second = daemon
        .post_json(
            "/v1/roots",
            &json!({ "path": second_link, "path_mode": "link" }),
        )
        .await;
    let second_id = second["id"].as_str().expect("an id").to_owned();

    retarget_link(&second_link, &real);

    let later = root_once(&daemon, &second_id, "the later Root overlapping", |root| {
        root["state_code"] == "root_overlaps"
    })
    .await;
    assert_eq!(later["state"], "unavailable", "{later}");
    let earlier = daemon.get_json(&format!("/v1/roots/{first_id}")).await;
    assert_eq!(earlier["state"], first["state"], "{earlier}");
    assert_eq!(earlier["state_code"], first["state_code"], "{earlier}");
    let captured = daemon
        .post(&format!("/v1/roots/{first_id}/snapshots"), &json!({}))
        .await;
    assert_eq!(captured.status, 201, "{}", body(&captured));
    let snapshot = body(&captured)["id"]
        .as_str()
        .expect("a Snapshot")
        .to_owned();
    let notes = daemon
        .get(&format!(
            "/v1/roots/{first_id}/snapshots/{snapshot}/content?path=notes.md"
        ))
        .await;
    assert_eq!(notes.body, b"the release", "the first keeps capturing");
}

#[tokio::test]
async fn a_link_root_follows_its_link_pointed_elsewhere_and_records_where_it_leads() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    std::fs::write(real.join("notes.md"), "the release").expect("a file");
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();
    let next = daemon.scratch("next");
    std::fs::write(next.join("notes.md"), "the next release").expect("a file");

    retarget_link(&link, &next);

    let root = root_once(&daemon, &id, "the new target recorded", |root| {
        root["real_path"] == next.to_string_lossy().as_ref()
    })
    .await;
    assert_eq!(root["path"], link.to_string_lossy().as_ref(), "{root}");
    assert_eq!(root["state"], created["state"], "{root}");
    assert_eq!(
        root["state_code"], created["state_code"],
        "no volume_mismatch: {root}"
    );
    let snapshot = daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await["id"]
        .as_str()
        .expect("a Snapshot")
        .to_owned();
    let notes = daemon
        .get(&format!(
            "/v1/roots/{id}/snapshots/{snapshot}/content?path=notes.md"
        ))
        .await;
    assert_eq!(
        notes.body, b"the next release",
        "captured from the new target"
    );
}

#[tokio::test]
async fn a_link_root_pointed_at_a_directory_holding_another_root_withholds_that_root() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, _) = linked(&daemon);
    let outer = daemon.scratch("outer");
    std::fs::write(outer.join("notes.md"), "outside").expect("a file");
    let inner = outer.join("inner");
    std::fs::create_dir(&inner).expect("a directory inside");
    std::fs::write(inner.join("secret.md"), "another Root's").expect("a file");
    daemon
        .post_json("/v1/roots", &json!({ "path": inner }))
        .await;
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();

    retarget_link(&link, &outer);
    root_once(&daemon, &id, "the new target recorded", |root| {
        root["real_path"] == outer.to_string_lossy().as_ref()
    })
    .await;
    let snapshot = daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await;

    let withheld = snapshot["withheld"].as_array().expect("withheld paths");
    assert!(
        withheld.contains(&json!({ "path": "inner", "reason": "other_root" })),
        "{snapshot}"
    );
    let snapshot = snapshot["id"].as_str().expect("a Snapshot");
    let notes = daemon
        .get(&format!(
            "/v1/roots/{id}/snapshots/{snapshot}/content?path=notes.md"
        ))
        .await;
    assert_eq!(
        notes.body, b"outside",
        "the rest of the new target is captured"
    );
}

#[tokio::test]
async fn a_link_root_pointed_at_a_directory_holding_the_history_store_withholds_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, _) = linked(&daemon);
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();
    // The daemon's own directory, which holds its store.
    let base = daemon
        .store_base()
        .parent()
        .expect("the daemon's directory")
        .to_path_buf();

    retarget_link(&link, &base);
    root_once(&daemon, &id, "the new target recorded", |root| {
        root["real_path"] == base.to_string_lossy().as_ref()
    })
    .await;
    let snapshot = daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await;

    let withheld = snapshot["withheld"].as_array().expect("withheld paths");
    assert!(
        withheld.contains(&json!({ "path": "store", "reason": "history_store" })),
        "{snapshot}"
    );
}

/// A quick debounce and rate limit, so a watched change is captured in moments.
const QUICK: &[u8] = b"[triggers]\ndebounce = \"200ms\"\nrate_limit = \"1s\"\n";

/// The newest Snapshot of the Root `id` that changed `path`, once one taken by `trigger` exists.
async fn snapshot_changing(daemon: &Harness, id: &str, path: &str, trigger: &str) -> Value {
    crate::cli_lifecycle::eventually(
        &format!("a `{trigger}` Snapshot changing `{path}`"),
        || async {
            let timeline = daemon
                .get_json(&format!(
                    "/v1/roots/{id}/snapshots?path={}",
                    keepster_api::encode_path_query(path)
                ))
                .await;
            timeline["snapshots"]
                .as_array()
                .and_then(|items| items.first())
                .filter(|newest| newest["trigger"] == trigger)
                .cloned()
        },
    )
    .await
}

#[tokio::test]
async fn a_link_root_pointed_elsewhere_is_reconciled_and_watched_there() {
    let daemon = Harness::start_with_global_config(QUICK).await;
    let (link, real) = linked(&daemon);
    std::fs::write(real.join("notes.md"), "the release").expect("a file");
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();
    snapshot_changing(&daemon, &id, "notes.md", "reconcile").await;
    let next = daemon.scratch("next");
    std::fs::write(next.join("only-next.md"), "the next release").expect("a file");

    retarget_link(&link, &next);

    // Started afresh on the new target, the runtime first passes over it: what it holds is captured
    // without anything changing there.
    snapshot_changing(&daemon, &id, "only-next.md", "reconcile").await;
    std::fs::write(next.join("fresh.md"), "written while watched").expect("a file");
    let taken = snapshot_changing(&daemon, &id, "fresh.md", "watcher").await;
    assert_eq!(taken["change"], "added", "{taken}");
}

#[tokio::test]
async fn a_restore_into_a_link_root_led_inside_another_root_is_refused_and_writes_nothing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);
    std::fs::write(real.join("notes.md"), "the release").expect("a file");
    let other = daemon.scratch("other");
    let inner = other.join("inner");
    std::fs::create_dir(&inner).expect("a directory inside");
    let existing = daemon
        .post_json("/v1/roots", &json!({ "path": other }))
        .await;
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();
    let snapshot = daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await["id"]
        .as_str()
        .expect("a Snapshot")
        .to_owned();

    retarget_link(&link, &inner);
    let refused = daemon
        .post(
            &format!("/v1/roots/{id}/restore"),
            &json!({ "from": snapshot, "paths": ["notes.md"] }),
        )
        .await;

    assert_eq!(refused.status, 409, "{}", body(&refused));
    let problem = body(&refused);
    assert_eq!(problem["code"], "root_overlaps", "{problem}");
    assert_eq!(problem["details"]["root"], existing["id"], "{problem}");
    assert_eq!(
        problem["details"]["path"],
        link.to_string_lossy().as_ref(),
        "{problem}"
    );
    assert!(
        !inner.join("notes.md").exists(),
        "nothing is written into the other Root's tree"
    );
}

#[tokio::test]
async fn a_capture_right_after_the_link_is_pointed_elsewhere_records_where_it_leads_first() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, _) = linked(&daemon);
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": link, "path_mode": "link" }))
        .await;
    let id = created["id"].as_str().expect("an id").to_owned();
    let next = daemon.scratch("next");

    // Without waiting for the reload tick: the capture's own look at the Root records the new target
    // before judging the volume, so nothing in between reads the old one.
    retarget_link(&link, &next);
    let captured = daemon
        .post(&format!("/v1/roots/{id}/snapshots"), &json!({}))
        .await;
    let root = daemon.get_json(&format!("/v1/roots/{id}")).await;

    assert_eq!(captured.status, 201, "{}", body(&captured));
    assert_eq!(
        root["real_path"],
        next.to_string_lossy().as_ref(),
        "re-recorded by the capture: {root}"
    );
}

#[tokio::test]
async fn the_cli_watching_a_link_without_a_flag_names_the_flags_and_registers_nothing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, _real) = linked(&daemon);

    let run = daemon
        .run_cli(KEEPSTER, &["watch", &link.to_string_lossy()])
        .await;

    // Nothing can be asked here (the test has no terminal), so the command says what it needs.
    assert_eq!(run.exit_code, Some(5), "stderr: {}", run.stderr);
    assert!(run.stderr.contains("--keep-link"), "stderr: {}", run.stderr);
    assert!(run.stderr.contains("--canonical"), "stderr: {}", run.stderr);
    assert_eq!(
        daemon.get_json("/v1/roots").await,
        json!([]),
        "nothing is registered until the choice is made"
    );
}

#[tokio::test]
async fn the_cli_watching_a_link_with_canonical_registers_the_real_path() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);

    let run = daemon
        .run_cli(KEEPSTER, &["watch", &link.to_string_lossy(), "--canonical"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let roots = daemon.get_json("/v1/roots").await;
    assert_eq!(roots[0]["path"], real.to_string_lossy().as_ref(), "{roots}");
    assert_eq!(roots[0]["path_mode"], "canonical", "{roots}");
    assert_eq!(roots.as_array().expect("a list").len(), 1);
}

#[tokio::test]
async fn the_cli_watching_a_link_with_keep_link_registers_the_link() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, real) = linked(&daemon);

    let run = daemon
        .run_cli(KEEPSTER, &["watch", &link.to_string_lossy(), "--keep-link"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let roots = daemon.get_json("/v1/roots").await;
    assert_eq!(roots[0]["path"], link.to_string_lossy().as_ref(), "{roots}");
    assert_eq!(roots[0]["path_mode"], "link", "{roots}");
    assert_eq!(
        roots[0]["real_path"],
        real.to_string_lossy().as_ref(),
        "{roots}"
    );
}

#[tokio::test]
async fn the_cli_refuses_keep_link_and_canonical_together_and_registers_nothing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (link, _real) = linked(&daemon);

    let run = daemon
        .run_cli(
            KEEPSTER,
            &[
                "watch",
                &link.to_string_lossy(),
                "--keep-link",
                "--canonical",
            ],
        )
        .await;

    // The command line itself is wrong: the usage exit code, before the daemon is asked anything.
    assert_eq!(run.exit_code, Some(2), "stderr: {}", run.stderr);
    assert!(run.stderr.contains("--keep-link"), "stderr: {}", run.stderr);
    assert!(run.stderr.contains("--canonical"), "stderr: {}", run.stderr);
    assert_eq!(daemon.get_json("/v1/roots").await, json!([]));
}

#[tokio::test]
async fn the_cli_moving_a_root_to_a_link_needs_a_flag_and_then_obeys_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let (link, real) = linked(&daemon);
    let created = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await;
    let id = created["id"].as_str().expect("an id");
    let link_text = link.to_string_lossy();

    let refused = daemon
        .run_cli(KEEPSTER, &["move", "-r", id, &link_text])
        .await;

    assert_eq!(refused.exit_code, Some(5), "stderr: {}", refused.stderr);
    assert!(
        refused.stderr.contains("--keep-link") && refused.stderr.contains("--canonical"),
        "stderr: {}",
        refused.stderr
    );
    assert_eq!(
        daemon.get_json("/v1/roots").await[0]["path"],
        project.to_string_lossy().as_ref(),
        "a refused move leaves the Root where it was"
    );

    let moved = daemon
        .run_cli(KEEPSTER, &["move", "-r", id, &link_text, "--canonical"])
        .await;

    assert_eq!(moved.exit_code, Some(0), "stderr: {}", moved.stderr);
    let roots = daemon.get_json("/v1/roots").await;
    assert_eq!(roots[0]["path"], real.to_string_lossy().as_ref(), "{roots}");
    assert_eq!(roots[0]["path_mode"], "canonical", "{roots}");
}
