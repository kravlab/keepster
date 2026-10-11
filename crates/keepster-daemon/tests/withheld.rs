//! What a Snapshot does not hold, at the `/v1` seam (ticket 48): Withheld paths listed with their reasons
//! and refused to Restore, incomplete paths named, the sidecar never shown, and an incomplete pass
//! retried until it is not.
//!
//! Unix only: what is withheld is made with `mkfifo`, and the daemon's own socket is one of them.
#![cfg(unix)]

use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::time::Duration;

use keepster_testkit::Harness;
use serde_json::{Value, json};

/// Nothing captures automatically, and files over 8 bytes are Withheld.
const SMALL_CAP: &[u8] =
    b"[triggers]\nactive = []\n\n[reconcile]\nat_start = false\n\n[files]\nmax_size = \"8B\"\n";

async fn registered(daemon: &Harness, project: &Path) -> String {
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await;

    root["id"].as_str().expect("an id").to_owned()
}

fn write(project: &Path, path: &str, content: &[u8]) {
    let file = project.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, content).expect("a file");
}

async fn take(daemon: &Harness, root: &str) -> Value {
    daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await
}

fn body(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

/// A Root holding a small file, one over the cap, a pipe in a directory of its own and a `.git`.
async fn withholding(daemon: &Harness) -> (String, PathBuf, String) {
    let project = daemon.scratch("project");
    write(&project, "notes.txt", b"small");
    write(&project, "big.bin", b"far over the cap");
    write(&project, ".git/HEAD", b"ref: refs/heads/main\n");
    std::fs::create_dir_all(project.join("run")).expect("a directory");
    let status = std::process::Command::new("mkfifo")
        .arg(project.join("run/control"))
        .status()
        .expect("mkfifo runs");
    assert!(status.success());

    let root = registered(daemon, &project).await;
    let snapshot = take(daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id").to_owned();

    (root, project, id)
}

#[tokio::test]
async fn entries_list_what_a_snapshot_withholds_and_why_and_never_its_sidecar() {
    let daemon = Harness::start_with_global_config(SMALL_CAP).await;
    let (root, _, snapshot) = withholding(&daemon).await;

    let top = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots/{snapshot}/entries"))
        .await;
    let listed: Vec<(&str, &str, Option<&str>)> = top["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("a path"),
                entry["kind"].as_str().expect("a kind"),
                entry["withheld"]["reason"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            (".git", "withheld", Some("reserved_name")),
            ("big.bin", "withheld", Some("too_large")),
            ("notes.txt", "file", None),
            ("run", "dir", None),
        ],
        "{top}"
    );

    let run = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/entries?path=run"
        ))
        .await;
    assert_eq!(
        run["entries"][0]["withheld"],
        json!({ "reason": "special_file", "kind": "fifo" })
    );
}

#[tokio::test]
async fn the_answer_to_a_capture_names_each_withheld_path_with_its_kind() {
    let daemon = Harness::start_with_global_config(SMALL_CAP).await;
    let project = daemon.scratch("project");
    std::fs::create_dir_all(project.join("run")).expect("a directory");
    let status = std::process::Command::new("mkfifo")
        .arg(project.join("run/control"))
        .status()
        .expect("mkfifo runs");
    assert!(status.success());
    let root = registered(&daemon, &project).await;

    let snapshot = take(&daemon, &root).await;

    assert_eq!(snapshot["incomplete"], false, "withholding is not failure");
    assert_eq!(
        snapshot["withheld"],
        json!([{ "path": "run/control", "reason": "special_file", "kind": "fifo" }])
    );
}

#[tokio::test]
async fn a_withheld_path_has_no_content_and_restoring_it_is_refused() {
    let daemon = Harness::start_with_global_config(SMALL_CAP).await;
    let (root, project, snapshot) = withholding(&daemon).await;

    let content = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/content?path=big.bin"
        ))
        .await;
    assert_eq!(content.status, 404);
    let problem = body(&content);
    assert_eq!(problem["code"], "withheld");
    assert_eq!(problem["details"]["reason"], "too_large");

    let sidecar = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/content?path=.keepster-meta"
        ))
        .await;
    assert_eq!(body(&sidecar)["code"], "path_not_found");

    let refused = daemon
        .post(
            &format!("/v1/roots/{root}/restore"),
            &json!({ "from": snapshot, "paths": ["big.bin"] }),
        )
        .await;
    assert_eq!(
        refused.status,
        404,
        "{}",
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(body(&refused)["code"], "withheld");

    std::fs::remove_file(project.join("notes.txt")).expect("deleted");
    let whole = daemon
        .post(
            &format!("/v1/roots/{root}/restore"),
            &json!({ "from": snapshot }),
        )
        .await;
    let answer = body(&whole);
    assert_eq!(whole.status, 200, "{answer}");
    let passed_over: Vec<(&str, &str, &str)> = answer["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .filter(|path| path["outcome"] == "skipped")
        .map(|path| {
            (
                path["path"].as_str().expect("a path"),
                path["issues"][0]["reason"].as_str().expect("a reason"),
                path["issues"][0]["why"].as_str().expect("why"),
            )
        })
        .collect();
    assert_eq!(
        passed_over,
        [
            (".git", "withheld", "reserved_name"),
            ("big.bin", "withheld", "too_large"),
            ("run/control", "withheld", "special_file"),
        ],
        "a Restore of the whole Root names what it cannot bring back"
    );
    assert!(
        !project.join(".keepster-meta").exists(),
        "the sidecar is never restored into the Root"
    );
    assert_eq!(
        std::fs::read(project.join("notes.txt")).expect("restored"),
        b"small"
    );
}

#[tokio::test]
async fn a_history_store_inside_a_root_is_withheld_as_one() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    // The daemon's own directory: its store, its configuration and its control socket.
    let base = daemon
        .store_dir("x")
        .parent()
        .and_then(Path::parent)
        .expect("the daemon's directory")
        .to_path_buf();
    let root = registered(&daemon, &base).await;

    let snapshot = take(&daemon, &root).await;

    let withheld = snapshot["withheld"].as_array().expect("withheld paths");
    assert!(
        withheld.contains(&json!({ "path": "store", "reason": "history_store" })),
        "{snapshot}"
    );
    assert!(
        withheld
            .iter()
            .any(|path| path["reason"] == "special_file" && path["kind"] == "socket"),
        "the control socket is a special file: {snapshot}"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_incomplete_pass_is_retried_until_it_is_whole() {
    use std::os::unix::ffi::OsStrExt;

    let daemon = Harness::start_with_retry_after(Duration::from_millis(500)).await;
    daemon.write_global_config(
        b"[triggers]\nactive = [\"periodic\"]\nperiodic = \"1h\"\nrate_limit = \"1s\"\n",
    );
    let project = daemon.scratch("project");
    write(&project, "notes.txt", b"kept");
    // A name that is not UTF-8 cannot be held until ticket 74, so every pass that meets it is
    // incomplete — whoever runs the test, unlike a file without read permission.
    let odd = project.join(std::ffi::OsStr::from_bytes(b"\xff.bin"));
    std::fs::write(&odd, b"x").expect("an odd name");
    let root = registered(&daemon, &project).await;

    let first = wait_for(&daemon, &root, |snapshot| snapshot["incomplete"] == true).await;
    assert_eq!(first["trigger"], "reconcile", "{first}");

    std::fs::remove_file(&odd).expect("removed, with no watcher to say so");

    let retried = wait_for(&daemon, &root, |snapshot| snapshot["incomplete"] == false).await;
    assert_eq!(
        retried["trigger"], "retry",
        "only a retry could have seen the file go: {retried}"
    );
}

/// The newest Snapshot once one satisfies `wanted`, waiting up to ten seconds.
#[cfg(target_os = "linux")]
async fn wait_for(daemon: &Harness, root: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let timeline = daemon
            .get_json(&format!("/v1/roots/{root}/snapshots"))
            .await;
        if let Some(newest) = timeline["snapshots"].get(0) {
            if wanted(newest) {
                return newest.clone();
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no such Snapshot came: {timeline}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
