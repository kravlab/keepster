//! `keepster restore` (ticket 47): the daemon's Restore from the command line, every confirmation and
//! resolution a flag, and a refusal that names the flag rather than a question nobody can answer.

use std::path::{Path, PathBuf};

use keepster_testkit::Harness;
use serde_json::Value;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

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

#[tokio::test]
async fn restore_puts_a_file_back_and_says_how_to_undo_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved");
    take(&daemon, &root).await;
    write(&project, "notes.txt", b"since");

    let run = daemon
        .run_cli_in(&project, KEEPSTER, &["restore", "notes.txt"])
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    assert_eq!(
        std::fs::read(project.join("notes.txt")).expect("the file"),
        b"saved"
    );
    let text = String::from_utf8(run.stdout).expect("text");
    assert!(text.contains("restored  notes.txt"), "{text}");
    let latest = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots?limit=1"))
        .await;
    let safety = latest["snapshots"][0]["id"].as_str().expect("an id");
    assert!(
        text.contains(&format!("keepster restore --from {safety}")),
        "the undo is spelled out: {text}"
    );
}

#[tokio::test]
async fn without_prompts_a_needed_confirmation_is_a_state_conflict_that_names_its_flag() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "deep/file.txt", b"saved");
    write(&project, "notes.txt", b"saved");
    take(&daemon, &root).await;
    std::fs::remove_dir_all(project.join("deep")).expect("removed");
    write(&project, "notes.txt", b"since");

    let refused = daemon
        .run_cli_in(&project, KEEPSTER, &["restore", ".", "--no-input"])
        .await;

    assert_eq!(refused.exit_code, Some(5), "{}", refused.stderr);
    assert!(
        refused.stderr.contains("deep/file.txt"),
        "{}",
        refused.stderr
    );
    assert!(
        refused.stderr.contains("--create-parents"),
        "{}",
        refused.stderr
    );
    assert_eq!(
        std::fs::read(project.join("notes.txt")).expect("the file"),
        b"since",
        "nothing was written"
    );

    let confirmed = daemon
        .run_cli_in(&project, KEEPSTER, &["restore", ".", "--create-parents"])
        .await;
    assert_eq!(confirmed.exit_code, Some(0), "{}", confirmed.stderr);
    assert_eq!(
        std::fs::read(project.join("deep/file.txt")).expect("the file"),
        b"saved"
    );
}

#[tokio::test]
async fn a_dry_run_prints_the_plan_and_writes_nothing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved");
    take(&daemon, &root).await;
    write(&project, "notes.txt", b"since");

    let run = daemon
        .run_cli_in(&project, KEEPSTER, &["restore", "notes.txt", "--dry-run"])
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let text = String::from_utf8(run.stdout).expect("text");
    assert!(text.contains("would restore  notes.txt"), "{text}");
    assert_eq!(
        std::fs::read(project.join("notes.txt")).expect("the file"),
        b"since"
    );
}

#[tokio::test]
async fn restore_as_json_is_the_daemons_answer() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved");
    let saved = take(&daemon, &root).await;
    write(&project, "notes.txt", b"since");

    let run = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &["restore", "notes.txt", "--from", &saved, "--json"],
        )
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let answer: Value = serde_json::from_slice(&run.stdout).expect("JSON");
    assert_eq!(answer["paths"][0]["outcome"], "restored");
    assert!(answer["safety_snapshot"].is_string());
}
