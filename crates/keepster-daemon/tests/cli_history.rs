//! `keepster log` and `keepster cat` as a person uses them, against a real daemon: the timeline and
//! one version's bytes, with every selector reduced to an ordinary timeline query.

use std::path::PathBuf;

use keepster_testkit::Harness;
use serde_json::Value;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

/// A Root whose `notes.md` went through three versions and was then deleted, with the Snapshots
/// oldest first.
async fn history(daemon: &Harness) -> (String, PathBuf, Vec<Value>) {
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = root["id"].as_str().expect("an id").to_owned();
    let url = format!("/v1/roots/{id}/snapshots");

    let mut taken = Vec::new();
    for version in ["first", "second", "third"] {
        std::fs::write(project.join("notes.md"), version).expect("a version");
        std::fs::write(project.join("other.txt"), version).expect("another file");
        taken.push(daemon.post_json(&url, &serde_json::json!({})).await);
    }
    std::fs::remove_file(project.join("notes.md")).expect("a deletion");
    taken.push(daemon.post_json(&url, &serde_json::json!({})).await);

    (id, project, taken)
}

fn id(snapshot: &Value) -> &str {
    snapshot["id"].as_str().expect("an id")
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).expect("text")
}

#[tokio::test]
async fn log_lists_the_roots_snapshots_newest_first() {
    let daemon = Harness::start().await;
    let (_, project, taken) = history(&daemon).await;

    let run = daemon.run_cli_in(&project, KEEPSTER, &["log"]).await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let printed = text(run.stdout);
    let order: Vec<&str> = printed
        .lines()
        .map(|line| line.split_whitespace().next().expect("an identity"))
        .collect();
    let expected: Vec<&str> = taken.iter().rev().map(id).collect();
    assert_eq!(order, expected, "in:\n{printed}");
    assert!(printed.contains("1 path changed"), "in:\n{printed}");
    assert!(printed.contains("manual"), "in:\n{printed}");
}

#[tokio::test]
async fn log_of_a_path_shows_how_each_snapshot_changed_it() {
    let daemon = Harness::start().await;
    let (_, project, taken) = history(&daemon).await;

    let run = daemon
        .run_cli_in(&project, KEEPSTER, &["log", "notes.md"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let printed = text(run.stdout);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines.len(), 4, "in:\n{printed}");
    assert!(lines[0].starts_with(id(&taken[3])) && lines[0].ends_with("deleted"));
    assert!(lines[1].ends_with("modified"), "in:\n{printed}");
    assert!(lines[3].starts_with(id(&taken[0])) && lines[3].ends_with("added"));
}

#[tokio::test]
async fn log_json_is_the_daemons_own_items() {
    let daemon = Harness::start().await;
    let (root, project, _) = history(&daemon).await;

    let run = daemon
        .run_cli_in(&project, KEEPSTER, &["log", "--json", "-n", "2"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let printed: Value = serde_json::from_slice(&run.stdout).expect("JSON");
    let served = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots?limit=2"))
        .await;
    assert_eq!(printed, served["snapshots"]);
}

#[tokio::test]
async fn log_bounds_by_moments_as_the_timeline_does() {
    let daemon = Harness::start().await;
    let (_, project, taken) = history(&daemon).await;
    let second_at = taken[1]["taken_at"].as_str().expect("a moment");

    let run = daemon
        .run_cli_in(&project, KEEPSTER, &["log", "--before", second_at])
        .await;
    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let printed = text(run.stdout);
    assert!(printed.contains(id(&taken[1])) && printed.contains(id(&taken[0])));
    assert!(!printed.contains(id(&taken[2])), "in:\n{printed}");

    let recent = daemon
        .run_cli_in(&project, KEEPSTER, &["log", "--after", "1h"])
        .await;
    assert_eq!(
        text(recent.stdout).lines().count(),
        4,
        "all of it is recent"
    );

    let refused = daemon
        .run_cli_in(&project, KEEPSTER, &["log", "--after", "soon"])
        .await;
    assert_eq!(
        refused.exit_code,
        Some(2),
        "a malformed moment is a usage error"
    );
}

#[tokio::test]
async fn cat_prints_the_latest_version_by_default_and_older_ones_by_selector() {
    let daemon = Harness::start().await;
    let (_, project, taken) = history(&daemon).await;
    let cat = |selector: String| {
        let project = project.clone();
        let daemon = &daemon;
        async move {
            daemon
                .run_cli_in(&project, KEEPSTER, &["cat", "other.txt", "-s", &selector])
                .await
        }
    };

    let latest = daemon
        .run_cli_in(&project, KEEPSTER, &["cat", "other.txt"])
        .await;
    assert_eq!(latest.exit_code, Some(0), "stderr: {}", latest.stderr);
    assert_eq!(text(latest.stdout), "third");

    assert_eq!(text(cat("latest".into()).await.stdout), "third");
    assert_eq!(text(cat("~0".into()).await.stdout), "third");
    assert_eq!(text(cat("~2".into()).await.stdout), "second");
    assert_eq!(text(cat(id(&taken[0]).to_owned()).await.stdout), "first");
    assert_eq!(
        text(
            cat(taken[1]["taken_at"].as_str().expect("a moment").to_owned())
                .await
                .stdout
        ),
        "second",
        "a moment selects the Snapshot current at that moment"
    );
    assert_eq!(text(cat("0s".into()).await.stdout), "third");

    let too_far = cat("~9".into()).await;
    assert_eq!(too_far.exit_code, Some(4), "stderr: {}", too_far.stderr);
    let too_early = cat("1h".into()).await;
    assert_eq!(
        too_early.exit_code,
        Some(4),
        "nothing was captured an hour ago"
    );
}

#[tokio::test]
async fn cat_reads_a_file_that_no_longer_exists() {
    let daemon = Harness::start().await;
    let (_, project, _) = history(&daemon).await;

    let run = daemon
        .run_cli_in(&project, KEEPSTER, &["cat", "notes.md", "-s", "~1"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    assert_eq!(
        text(run.stdout),
        "third",
        "the version just before its deletion"
    );

    let gone = daemon
        .run_cli_in(&project, KEEPSTER, &["cat", "notes.md"])
        .await;
    assert_eq!(
        gone.exit_code,
        Some(4),
        "the newest Snapshot holds no such file"
    );
    assert!(gone.stderr.contains("path_not_found"), "{}", gone.stderr);
}

#[tokio::test]
async fn cat_of_something_that_is_not_a_file_is_a_usage_error() {
    let daemon = Harness::start().await;
    let (_, project, _) = history(&daemon).await;

    let root = daemon.run_cli_in(&project, KEEPSTER, &["cat", "."]).await;
    assert_eq!(root.exit_code, Some(2), "stderr: {}", root.stderr);

    let selector = daemon
        .run_cli_in(&project, KEEPSTER, &["cat", "other.txt", "-s", "yesterday"])
        .await;
    assert_eq!(selector.exit_code, Some(2), "stderr: {}", selector.stderr);
}
