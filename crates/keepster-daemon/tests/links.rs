//! Symlinks at the `/v1` seam (ticket 49): stored and restored as links, followed only where the user's
//! own configuration says, and the followed targets covered by a pass on their own interval.
#![cfg(unix)]

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::Duration;

use keepster_testkit::Harness;
use serde_json::{Value, json};

async fn registered(daemon: &Harness, project: &Path) -> String {
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await;

    root["id"].as_str().expect("an id").to_owned()
}

async fn take(daemon: &Harness, root: &str) -> String {
    let taken = daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await;

    taken["id"].as_str().expect("an id").to_owned()
}

async fn entries(daemon: &Harness, root: &str, snapshot: &str) -> Vec<(String, String)> {
    let listed = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/entries?depth=infinity"
        ))
        .await;

    listed["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("a path").to_owned(),
                entry["kind"].as_str().expect("a kind").to_owned(),
            )
        })
        .collect()
}

/// A Root with a file, and a link to a directory outside it holding one file.
fn linked(daemon: &Harness) -> (PathBuf, PathBuf) {
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.txt"), b"notes").expect("a file");
    let shared = daemon.scratch("shared");
    std::fs::write(shared.join("plan.md"), b"plan").expect("a file outside the Root");
    symlink(&shared, project.join("team")).expect("a link");

    (project, shared)
}

#[tokio::test]
async fn a_link_is_listed_and_restored_as_a_link() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (project, shared) = linked(&daemon);
    let root = registered(&daemon, &project).await;
    let snapshot = take(&daemon, &root).await;

    assert_eq!(
        entries(&daemon, &root, &snapshot).await,
        [
            ("notes.txt".to_owned(), "file".to_owned()),
            ("team".to_owned(), "symlink".to_owned()),
        ]
    );
    let content = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/content?path=team"
        ))
        .await;
    assert_eq!(content.body, shared.as_os_str().as_encoded_bytes());

    std::fs::remove_file(project.join("team")).expect("deleted");
    let restored = daemon
        .post(
            &format!("/v1/roots/{root}/restore"),
            &json!({ "from": snapshot, "paths": ["team"] }),
        )
        .await;
    assert_eq!(
        restored.status,
        200,
        "{}",
        String::from_utf8_lossy(&restored.body)
    );
    assert_eq!(
        std::fs::read_link(project.join("team")).expect("a link again"),
        shared
    );
}

#[tokio::test]
async fn a_root_s_own_file_cannot_make_its_links_followed() {
    // What may enter history through a link is the user's to decide: a `.keepster.toml` arriving with
    // someone else's repository must not pull `~/.ssh` in (tickets 04 and 49).
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (project, _) = linked(&daemon);
    std::fs::write(
        project.join(".keepster.toml"),
        b"[files]\nfollow_symlinks = [\"**\"]\n",
    )
    .expect("the Root's own file");
    let root = registered(&daemon, &project).await;

    let snapshot = take(&daemon, &root).await;

    assert!(
        entries(&daemon, &root, &snapshot)
            .await
            .contains(&("team".to_owned(), "symlink".to_owned()))
    );
}

#[tokio::test]
async fn a_followed_link_is_captured_through_and_its_targets_covered_on_their_own_interval() {
    let daemon = Harness::start_with_global_config(
        b"[files]\nfollow_symlinks = [\"team\"]\nfollow_reconcile_interval = \"1s\"\n\n\
          [triggers]\nactive = [\"periodic\"]\nperiodic = \"1h\"\nrate_limit = \"1s\"\n",
    )
    .await;
    let (project, shared) = linked(&daemon);
    let root = registered(&daemon, &project).await;

    let first = newest_holding(&daemon, &root, "team/plan.md", b"plan").await;
    let shown = daemon.get_json(&format!("/v1/roots/{root}")).await;
    assert_eq!(shown["follow_targets"]["count"], 1, "{shown}");
    assert_eq!(shown["follow_targets"]["interval"], 1, "{shown}");
    assert!(
        shown["follow_targets"]["last_reconciled_at"].is_string(),
        "{shown}"
    );

    // No watch covers the target, and the periodic floor is an hour away: only the followed targets'
    // own pass can see this.
    std::fs::write(shared.join("plan.md"), b"replanned").expect("an edit outside the Root");
    let second = newest_holding(&daemon, &root, "team/plan.md", b"replanned").await;
    assert_ne!(first, second);
}

/// The newest Snapshot once it holds `content` at `path`, waiting up to ten seconds.
async fn newest_holding(daemon: &Harness, root: &str, path: &str, content: &[u8]) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let timeline: Value = daemon
            .get_json(&format!("/v1/roots/{root}/snapshots?limit=1"))
            .await;
        if let Some(id) = timeline["snapshots"][0]["id"].as_str() {
            let held = daemon
                .get(&format!(
                    "/v1/roots/{root}/snapshots/{id}/content?path={path}"
                ))
                .await;
            if held.status == 200 && held.body == content {
                return id.to_owned();
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no Snapshot came holding {path}: {timeline}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
