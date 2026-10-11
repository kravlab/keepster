//! `keepster diff` (ticket 46): the daemon's diff, printed — a summary of changed paths, or a file's
//! unified hunks — and written to a file byte for byte when the shell would re-encode it.

use std::path::Path;

use keepster_testkit::Harness;
use serde_json::Value;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

async fn registered(daemon: &Harness) -> (String, std::path::PathBuf) {
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    (root["id"].as_str().expect("an id").to_owned(), project)
}

fn write(project: &Path, path: &str, content: &[u8]) {
    std::fs::write(project.join(path), content).expect("a file");
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

#[tokio::test]
async fn diff_compares_the_latest_snapshot_with_the_working_tree_by_default() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved\n");
    write(&project, "gone.txt", b"bye\n");
    take(&daemon, &root).await;
    write(&project, "notes.txt", b"edited\n");
    std::fs::remove_file(project.join("gone.txt")).expect("removed");
    write(&project, "new.txt", b"hello\n");

    let run = daemon.run_cli_in(&project, KEEPSTER, &["diff"]).await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    assert_eq!(
        String::from_utf8(run.stdout).expect("text"),
        "D  gone.txt\nA  new.txt\nM  notes.txt\n"
    );
}

#[tokio::test]
async fn diff_of_a_file_between_two_snapshots_prints_its_unified_hunks() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"one\ntwo\n");
    take(&daemon, &root).await;
    write(&project, "notes.txt", b"one\nthree\n");
    take(&daemon, &root).await;
    write(&project, "notes.txt", b"not compared\n");

    let run = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &["diff", "notes.txt", "--from", "~1", "--to", "latest"],
        )
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let text = String::from_utf8(run.stdout).expect("text");
    assert!(
        text.ends_with("@@ -1,2 +1,2 @@\n one\n-two\n+three\n"),
        "{text}"
    );
}

#[tokio::test]
async fn diff_writes_the_daemons_exact_bytes_to_a_file() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"caf\xe9\r\nline\n");
    let before = take(&daemon, &root).await;
    write(&project, "notes.txt", b"caf\xe9\r\nline changed\n");
    write(&project, "other.txt", b"also\n");
    let after = take(&daemon, &root).await;
    let patch = daemon.scratch("out").join("whole.patch");

    let run = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &[
                "diff",
                "--patch",
                "--from",
                &before,
                "--to",
                &after,
                "-o",
                patch.to_str().expect("UTF-8"),
            ],
        )
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    assert!(run.stdout.is_empty(), "nothing on standard output");
    let served = daemon
        .get(&format!(
            "/v1/roots/{root}/diff?from={before}&to={after}&format=unified"
        ))
        .await;
    assert_eq!(std::fs::read(&patch).expect("the file"), served.body);
    assert!(
        served.body.windows(4).any(|bytes| bytes == b"caf\xe9"),
        "not re-encoded on the way"
    );
}

#[tokio::test]
async fn diff_as_json_is_the_daemons_answer() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"one\n");
    take(&daemon, &root).await;
    write(&project, "notes.txt", b"two\n");

    let run = daemon
        .run_cli_in(&project, KEEPSTER, &["diff", "--json"])
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let answer: Value = serde_json::from_slice(&run.stdout).expect("JSON");
    assert_eq!(answer["to"], "working");
    assert_eq!(answer["files"][0]["change"], "modified");
}
