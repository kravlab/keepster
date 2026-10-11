//! Reading history back at the `/v1` seam: the timeline, a Snapshot's entries and one version's bytes,
//! and the derived index under them surviving deletion and a daemon killed mid-write.

use std::path::{Path, PathBuf};

use keepster_testkit::Harness;
use serde_json::Value;

/// A registered Root with a file in it.
async fn registered(daemon: &Harness) -> (String, PathBuf) {
    let project = daemon.scratch("project");
    std::fs::create_dir_all(project.join("src")).expect("a subdirectory");
    std::fs::write(project.join("a.txt"), "one").expect("a file");
    std::fs::write(project.join("src/lib.rs"), "fn main() {}\n").expect("a nested file");

    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    (root["id"].as_str().expect("an id").to_owned(), project)
}

async fn take(daemon: &Harness, root: &str) -> Value {
    daemon
        .post_json(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await
}

fn ids(page: &Value) -> Vec<String> {
    page["snapshots"]
        .as_array()
        .expect("the snapshots")
        .iter()
        .map(|snapshot| snapshot["id"].as_str().expect("an id").to_owned())
        .collect()
}

fn id(snapshot: &Value) -> String {
    snapshot["id"].as_str().expect("an id").to_owned()
}

fn problem(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("a problem document")
}

/// Three Snapshots: everything added; one file modified; that file deleted.
async fn three(daemon: &Harness, root: &str, project: &Path) -> [Value; 3] {
    let first = take(daemon, root).await;
    std::fs::write(project.join("a.txt"), "two").expect("an edit");
    let second = take(daemon, root).await;
    std::fs::remove_file(project.join("a.txt")).expect("a deletion");
    let third = take(daemon, root).await;

    [first, second, third]
}

#[tokio::test]
async fn the_timeline_lists_snapshots_newest_first_in_the_documented_shape() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    let [first, second, third] = three(&daemon, &root, &project).await;

    let page = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;

    assert_eq!(ids(&page), [id(&third), id(&second), id(&first)]);
    assert!(page.get("next_cursor").is_none(), "one page holds it all");

    let newest = &page["snapshots"][0];
    assert_eq!(newest["trigger"], "manual");
    assert_eq!(newest["incomplete"], false);
    assert_eq!(newest["changed_paths"], 1, "it deleted one file");
    assert_eq!(newest["segment"], third["segment"]);
    assert_eq!(
        newest["taken_at"], third["taken_at"],
        "the timeline and the capture agree on the moment"
    );
    assert!(newest.get("change").is_none(), "no path was asked about");
    assert_eq!(
        page["snapshots"][2]["changed_paths"], 2,
        "the first added both"
    );
}

#[tokio::test]
async fn pages_follow_by_cursor_and_a_cursor_to_a_vanished_snapshot_is_not_an_error() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    let [first, second, third] = three(&daemon, &root, &project).await;

    let page = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots?limit=2"))
        .await;
    assert_eq!(ids(&page), [id(&third), id(&second)]);
    let cursor = page["next_cursor"].as_str().expect("a next cursor");

    let rest = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots?limit=2&cursor={cursor}"
        ))
        .await;
    assert_eq!(ids(&rest), [id(&first)]);
    assert!(rest.get("next_cursor").is_none(), "the last page says so");

    // An identity no Snapshot has — as one Retention removed would be — still marks a place in the
    // order. The newest possible identity marks the place above everything.
    let vanished = "7ZZZZZZZZZZZZZZZZZZZZZZZZZ";
    let all = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots?cursor={vanished}"))
        .await;
    assert_eq!(ids(&all), [id(&third), id(&second), id(&first)]);
}

#[tokio::test]
async fn filtering_by_a_path_returns_the_snapshots_that_changed_it_with_how() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    let [first, second, third] = three(&daemon, &root, &project).await;

    let page = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots?path=a.txt"))
        .await;

    assert_eq!(ids(&page), [id(&third), id(&second), id(&first)]);
    let changes: Vec<_> = page["snapshots"]
        .as_array()
        .expect("the snapshots")
        .iter()
        .map(|snapshot| snapshot["change"].as_str().expect("a change"))
        .collect();
    assert_eq!(changes, ["deleted", "modified", "added"]);

    let once = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots?path=src/lib.rs"))
        .await;
    assert_eq!(ids(&once), [id(&first)]);
}

#[tokio::test]
async fn moments_bound_the_timeline_at_or_before_and_strictly_after() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    let [first, second, third] = three(&daemon, &root, &project).await;
    let at = |snapshot: &Value| snapshot["taken_at"].as_str().expect("a moment").to_owned();

    let before = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots?before={}",
            at(&second)
        ))
        .await;
    let before = ids(&before);
    assert!(
        before.contains(&id(&second)),
        "at the moment counts as before it"
    );
    assert!(!before.contains(&id(&third)));

    let after = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots?after={}", at(&first)))
        .await;
    let after = ids(&after);
    assert!(!after.contains(&id(&first)), "after is strictly after");
    assert!(after.contains(&id(&third)));
}

#[tokio::test]
async fn malformed_timeline_parameters_are_refused_by_name() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, _) = registered(&daemon).await;

    for (query, code) in [
        ("limit=0", "limit_invalid"),
        ("limit=many", "limit_invalid"),
        ("cursor=nope", "cursor_invalid"),
        ("before=yesterday", "moment_invalid"),
        ("path=../etc/passwd", "path_invalid"),
        ("path=/etc/passwd", "path_invalid"),
    ] {
        let answer = daemon
            .get(&format!("/v1/roots/{root}/snapshots?{query}"))
            .await;
        assert_eq!(answer.status, 400, "{query}");
        assert_eq!(problem(&answer.body)["code"], code, "{query}");
    }
}

#[tokio::test]
async fn a_root_never_captured_has_an_empty_timeline_and_no_store() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, _) = registered(&daemon).await;

    let page = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;

    assert_eq!(ids(&page), Vec::<String>::new());
    assert!(
        !daemon.store_dir(&root).join("history.git").exists(),
        "reading created no History Store"
    );
}

#[tokio::test]
async fn entries_are_shallow_by_default_and_complete_with_depth_infinity() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    std::fs::write(project.join("src/data.bin"), [0u8, 1, 2, 3]).expect("a binary file");
    let snapshot = id(&take(&daemon, &root).await);
    let base = format!("/v1/roots/{root}/snapshots/{snapshot}/entries");

    // What the umask gave each path is ticket 50's to test; here it is only the shape.
    let without_permissions = |mut listing: serde_json::Value| {
        for entry in listing["entries"].as_array_mut().expect("entries") {
            entry
                .as_object_mut()
                .expect("an entry")
                .remove("permissions");
        }
        listing["entries"].take()
    };
    let shallow = daemon.get_json(&base).await;
    assert_eq!(
        without_permissions(shallow),
        serde_json::json!([
            { "path": "a.txt", "kind": "file", "size": 3, "mode": 420, "binary": false },
            { "path": "src", "kind": "dir" },
        ])
    );

    let full = daemon
        .get_json(&format!("{base}?path=src&depth=infinity"))
        .await;
    assert_eq!(
        without_permissions(full),
        serde_json::json!([
            { "path": "src/data.bin", "kind": "file", "size": 4, "mode": 420, "binary": true },
            { "path": "src/lib.rs", "kind": "file", "size": 13, "mode": 420, "binary": false },
        ])
    );

    let refused = daemon.get(&format!("{base}?depth=2")).await;
    assert_eq!(refused.status, 400);
    assert_eq!(problem(&refused.body)["code"], "depth_invalid");

    let not_a_directory = daemon.get(&format!("{base}?path=a.txt")).await;
    assert_eq!(not_a_directory.status, 409);
    assert_eq!(problem(&not_a_directory.body)["code"], "not_a_directory");

    let unknown = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/01ARZ3NDEKTSV4RRFFQ69G5FAV/entries"
        ))
        .await;
    assert_eq!(unknown.status, 404);
    assert_eq!(problem(&unknown.body)["code"], "snapshot_not_found");
}

#[tokio::test]
async fn content_is_raw_bytes_with_a_length_a_tag_and_ranges() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    let body: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
    std::fs::write(project.join("big.bin"), &body).expect("a large file");
    let old = id(&take(&daemon, &root).await);
    std::fs::write(project.join("big.bin"), "replaced").expect("an edit");
    take(&daemon, &root).await;
    let url = format!("/v1/roots/{root}/snapshots/{old}/content?path=big.bin");

    let whole = daemon.get(&url).await;
    assert_eq!(whole.status, 200);
    assert_eq!(whole.body, body, "the old version, byte for byte");
    assert_eq!(whole.headers["content-length"], body.len().to_string());
    assert_eq!(whole.headers["content-type"], "application/octet-stream");
    assert_eq!(whole.headers["accept-ranges"], "bytes");
    let tag = whole.headers["etag"].to_str().expect("a tag").to_owned();
    assert!(
        tag.starts_with('"') && tag.ends_with('"'),
        "a strong tag: {tag}"
    );

    let ranged = daemon
        .get_with(&url, &[("range", "bytes=250000-250009")])
        .await;
    assert_eq!(ranged.status, 206);
    assert_eq!(
        ranged.body,
        &body[250_000..250_010],
        "only the range is in the response"
    );
    assert_eq!(ranged.headers["content-length"], "10");
    assert_eq!(
        ranged.headers["content-range"],
        format!("bytes 250000-250009/{}", body.len())
    );

    let tail = daemon.get_with(&url, &[("range", "bytes=-5")]).await;
    assert_eq!(tail.status, 206);
    assert_eq!(tail.body, &body[body.len() - 5..]);

    let outside = daemon.get_with(&url, &[("range", "bytes=400000-")]).await;
    assert_eq!(outside.status, 416);
    assert_eq!(
        outside.headers["content-range"],
        format!("bytes */{}", body.len())
    );

    let cached = daemon.get_with(&url, &[("if-none-match", &tag)]).await;
    assert_eq!(cached.status, 304, "the client already holds this content");
    assert!(cached.body.is_empty());
}

#[tokio::test]
async fn content_refuses_what_is_not_a_file_by_name() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, _) = registered(&daemon).await;
    let snapshot = id(&take(&daemon, &root).await);
    let base = format!("/v1/roots/{root}/snapshots/{snapshot}/content");

    for (query, status, code) in [
        ("?path=src", 409, "not_a_file"),
        ("?path=missing.txt", 404, "path_not_found"),
        ("", 409, "not_a_file"),
        ("?path=./a.txt", 400, "path_invalid"),
    ] {
        let answer = daemon.get(&format!("{base}{query}")).await;
        assert_eq!(answer.status, status, "{query}");
        assert_eq!(problem(&answer.body)["code"], code, "{query}");
    }
}

/// The index file and its companions, which are created and deleted as one set (ticket 03).
fn index_set(daemon: &Harness, root: &str) -> Vec<PathBuf> {
    ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| daemon.store_dir(root).join(format!("index.sqlite{suffix}")))
        .collect()
}

#[tokio::test]
async fn deleting_the_index_and_restarting_serves_the_same_timeline() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    three(&daemon, &root, &project).await;
    let url = format!("/v1/roots/{root}/snapshots");
    let filtered = format!("{url}?path=a.txt");
    let before = (
        daemon.get_json(&url).await,
        daemon.get_json(&filtered).await,
    );

    let daemon = daemon.restart().await;
    for file in index_set(&daemon, &root) {
        let _ = std::fs::remove_file(file);
    }
    let daemon = daemon.restart().await;

    assert!(
        daemon.store_dir(&root).join("index.sqlite").is_file(),
        "the start rebuilt the index"
    );
    let after = (
        daemon.get_json(&url).await,
        daemon.get_json(&filtered).await,
    );
    assert_eq!(after, before);
}

#[tokio::test]
async fn a_daemon_killed_between_ref_update_and_index_write_repairs_itself_at_start() {
    // The index write is a capture's last step, so a kill between the ref update and that write leaves
    // exactly the index as it was before the capture. Restoring a copy taken before it reproduces that
    // state without fabricating anything in git.
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    let kept = take(&daemon, &root).await;

    let daemon = daemon.restart().await;
    let saved = daemon.scratch("saved");
    let index = daemon.store_dir(&root).join("index.sqlite");
    std::fs::copy(&index, saved.join("index.sqlite")).expect("a copy of the index");

    std::fs::write(project.join("a.txt"), "two").expect("an edit");
    let lost = take(&daemon, &root).await;

    let daemon = daemon.restart().await;
    for file in index_set(&daemon, &root) {
        let _ = std::fs::remove_file(file);
    }
    std::fs::copy(saved.join("index.sqlite"), &index).expect("the index as the kill left it");
    assert_eq!(indexed(&index), [id(&kept)], "lagging, never ahead");
    let daemon = daemon.restart().await;

    // Repaired by the start itself, before any request could have caught it up on the way.
    assert_eq!(indexed(&index), [id(&kept), id(&lost)]);

    let page = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(ids(&page), [id(&lost), id(&kept)]);
    assert_eq!(page["snapshots"][0]["changed_paths"], 1);
}

/// The Snapshot identities the index holds, oldest first, read straight from the file.
fn indexed(index: &Path) -> Vec<String> {
    let connection = rusqlite::Connection::open(index).expect("the index");
    let mut statement = connection
        .prepare("SELECT id FROM snapshots ORDER BY id")
        .expect("a query");
    statement
        .query_map([], |row| row.get(0))
        .expect("the rows")
        .collect::<Result<_, _>>()
        .expect("their values")
}
