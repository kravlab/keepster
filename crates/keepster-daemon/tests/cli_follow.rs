//! `keepster log --follow` (ticket 45): the timeline's tail, oldest first, then every Snapshot as it is
//! taken — read off the very event stream a browser uses, over the socket.

use std::path::Path;
use std::time::Duration;

use keepster_testkit::Harness;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader, Lines};
use tokio::process::ChildStdout;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

/// Long enough for a loaded CI runner; each test finishes as soon as the line it waits for is printed.
const PATIENCE: Duration = Duration::from_secs(30);

async fn register(daemon: &Harness, project: &Path) -> String {
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    root["id"].as_str().expect("an id").to_owned()
}

/// Write `content` to `notes.txt` and take a Snapshot of it by hand, returning its identity.
async fn edit_and_take(daemon: &Harness, root: &str, project: &Path, content: &str) -> String {
    std::fs::write(project.join("notes.txt"), content).expect("an edit");
    let taken = daemon
        .post_json(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await;

    taken["id"].as_str().expect("an id").to_owned()
}

async fn line(lines: &mut Lines<BufReader<ChildStdout>>) -> String {
    tokio::time::timeout(PATIENCE, lines.next_line())
        .await
        .unwrap_or_else(|_| panic!("nothing printed within {PATIENCE:?}"))
        .expect("standard output reads")
        .expect("still following")
}

fn follow(
    daemon: &Harness,
    args: &[&str],
) -> (tokio::process::Child, Lines<BufReader<ChildStdout>>) {
    let mut child = daemon.spawn_cli(KEEPSTER, args);
    let stdout = child.stdout.take().expect("piped");

    (child, BufReader::new(stdout).lines())
}

#[tokio::test]
async fn follow_prints_the_tail_oldest_first_then_each_snapshot_as_it_is_taken() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let root = register(&daemon, &project).await;
    let first = edit_and_take(&daemon, &root, &project, "one").await;
    let second = edit_and_take(&daemon, &root, &project, "two").await;
    let third = edit_and_take(&daemon, &root, &project, "three").await;

    let (_child, mut lines) = follow(&daemon, &["log", "--follow", "-n", "2", "-r", &root]);

    assert!(
        line(&mut lines).await.starts_with(&second),
        "the tail starts with the older of the two"
    );
    assert!(line(&mut lines).await.starts_with(&third));
    let fourth = edit_and_take(&daemon, &root, &project, "four").await;
    let printed = line(&mut lines).await;
    assert!(printed.starts_with(&fourth), "{printed}");
    assert!(printed.contains("manual"), "{printed}");
    assert!(!printed.contains(&first));
}

#[tokio::test]
async fn following_a_path_prints_only_the_snapshots_that_changed_it_as_json_lines() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    std::fs::write(project.join("other.txt"), "untouched").expect("a file");
    let root = register(&daemon, &project).await;
    edit_and_take(&daemon, &root, &project, "one").await;

    let notes = project.join("notes.txt");
    let (_child, mut lines) = follow(
        &daemon,
        &["log", "--follow", "--json", notes.to_str().expect("UTF-8")],
    );
    let tail: Value =
        serde_json::from_str(&line(&mut lines).await).expect("one JSON object per line");
    assert_eq!(tail["change"], "added");

    std::fs::write(project.join("other.txt"), "touched").expect("an edit elsewhere");
    daemon
        .post_json(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await;
    let changed = edit_and_take(&daemon, &root, &project, "two").await;

    let next: Value = serde_json::from_str(&line(&mut lines).await).expect("JSON");
    assert_eq!(
        next["id"],
        changed.as_str(),
        "the Snapshot that left notes.txt alone is not printed"
    );
    assert_eq!(next["change"], "modified");
}

#[tokio::test]
async fn following_with_no_tail_prints_only_what_is_new() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let root = register(&daemon, &project).await;
    edit_and_take(&daemon, &root, &project, "one").await;
    edit_and_take(&daemon, &root, &project, "two").await;

    let (_child, mut lines) = follow(&daemon, &["log", "--follow", "-n", "0", "-r", &root]);

    // Nothing is printed until following starts, so Snapshots are taken until one is: whichever it is, it
    // is one of these, never the history before them.
    let mut new = Vec::new();
    let printed = loop {
        new.push(edit_and_take(&daemon, &root, &project, &format!("edit {}", new.len())).await);
        if let Ok(printed) = tokio::time::timeout(Duration::from_secs(1), lines.next_line()).await {
            break printed
                .expect("standard output reads")
                .expect("still following");
        }
        assert!(new.len() < 30, "nothing printed while following");
    };
    assert!(
        new.iter().any(|id| printed.starts_with(id)),
        "the history before it is not printed: {printed}"
    );
}
