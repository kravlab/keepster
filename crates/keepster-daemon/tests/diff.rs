//! Diffs at the `/v1` seam (ticket 46), against a real store: the daemon computes them, so every client
//! shows the same summary and the same hunks.

use std::path::{Path, PathBuf};

use keepster_testkit::Harness;
use serde_json::Value;

async fn registered(daemon: &Harness) -> (String, PathBuf) {
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    (root["id"].as_str().expect("an id").to_owned(), project)
}

fn write(project: &Path, path: &str, content: &[u8]) {
    let file = project.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, content).expect("a file");
}

async fn take(daemon: &Harness, root: &str) -> String {
    let taken = daemon
        .post_json(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await;

    taken["id"].as_str().expect("an id").to_owned()
}

fn problem_code(body: &[u8]) -> String {
    let problem: Value = serde_json::from_slice(body).expect("a problem document");
    problem["code"].as_str().expect("a code").to_owned()
}

#[tokio::test]
async fn two_snapshots_differ_by_a_summary_and_a_path_by_its_hunks() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"one\ntwo\n");
    write(&project, "logo.png", b"\x89PNG\0one");
    let body = b"enough text that a moved file is recognised by its content\n".repeat(10);
    write(&project, "old/moved.txt", &body);
    let before = take(&daemon, &root).await;
    write(&project, "notes.txt", b"one\nthree\n");
    write(&project, "logo.png", b"\x89PNG\0two");
    std::fs::remove_file(project.join("old/moved.txt")).expect("moved away");
    write(&project, "new/moved.txt", &body);
    let after = take(&daemon, &root).await;

    let summary = daemon
        .get_json(&format!("/v1/roots/{root}/diff?from={before}&to={after}"))
        .await;

    assert_eq!(
        summary,
        serde_json::json!({
            "from": before,
            "to": after,
            "files": [
                { "path": "logo.png", "change": "modified", "binary": true, "too_large": false },
                { "path": "new/moved.txt", "old_path": "old/moved.txt", "change": "renamed", "binary": false, "too_large": false },
                { "path": "notes.txt", "change": "modified", "binary": false, "too_large": false },
            ],
        })
    );

    let hunks = daemon
        .get_json(&format!(
            "/v1/roots/{root}/diff?from={before}&to={after}&path=notes.txt"
        ))
        .await;
    assert_eq!(
        hunks["files"][0]["hunks"],
        serde_json::json!([{
            "old_start": 1, "old_lines": 2, "new_start": 1, "new_lines": 2,
            "lines": [
                { "kind": "context", "text": "one" },
                { "kind": "deleted", "text": "two" },
                { "kind": "added", "text": "three" },
            ],
        }])
    );
}

#[tokio::test]
async fn the_working_tree_is_a_side_to_compare_with() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved\n");
    let saved = take(&daemon, &root).await;
    write(&project, "notes.txt", b"since edited\n");

    let summary = daemon
        .get_json(&format!("/v1/roots/{root}/diff?from={saved}&to=working"))
        .await;

    assert_eq!(summary["to"], "working");
    assert_eq!(summary["files"][0]["path"], "notes.txt");
    assert_eq!(summary["files"][0]["change"], "modified");
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(
        timeline["snapshots"].as_array().map(Vec::len),
        Some(1),
        "comparing with the Working Tree takes no Snapshot"
    );
}

#[tokio::test]
async fn the_unified_format_is_the_patch_text_itself() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"one\ntwo\n");
    let before = take(&daemon, &root).await;
    write(&project, "notes.txt", b"one\nthree\n");
    let after = take(&daemon, &root).await;

    let patch = daemon
        .get(&format!(
            "/v1/roots/{root}/diff?from={before}&to={after}&path=notes.txt&format=unified"
        ))
        .await;

    assert_eq!(patch.status, 200);
    assert_eq!(
        patch
            .headers
            .get("content-type")
            .map(|value| value.as_bytes()),
        Some(&b"text/x-diff"[..])
    );
    let text = String::from_utf8(patch.body).expect("text");
    assert!(
        text.ends_with("--- a/notes.txt\n+++ b/notes.txt\n@@ -1,2 +1,2 @@\n one\n-two\n+three\n"),
        "{text}"
    );
}

#[tokio::test]
async fn a_comparison_that_cannot_be_made_is_refused_with_a_reason() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"one\n");
    let taken = take(&daemon, &root).await;

    for (query, status, code) in [
        (format!("from={taken}"), 400, "comparison_invalid"),
        (
            format!("from=yesterday&to={taken}"),
            400,
            "comparison_invalid",
        ),
        (
            format!("from={taken}&to={taken}&format=html"),
            400,
            "format_invalid",
        ),
        (
            format!("from=01K6DX3QR7M2V8C9WZ4PN5YQ1A&to={taken}"),
            404,
            "snapshot_not_found",
        ),
        (
            format!("from={taken}&to=working&path=nowhere.txt"),
            404,
            "path_not_found",
        ),
    ] {
        let answer = daemon.get(&format!("/v1/roots/{root}/diff?{query}")).await;
        assert_eq!(answer.status, status, "{query}");
        assert_eq!(problem_code(&answer.body), code, "{query}");
    }
}
