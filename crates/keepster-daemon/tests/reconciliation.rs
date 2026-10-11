//! Reconciliation at the `/v1` seam (ticket 44): the pass the coverage promise rests on, at the
//! daemon's start, after a reload that changed the rules and on request — proven with no watcher at all
//! wherever a watcher could have done the work instead.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use keepster_testkit::Harness;
use serde_json::Value;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

/// Long enough for a loaded CI runner; each test finishes as soon as what it waits for happens.
const PATIENCE: Duration = Duration::from_secs(30);

/// No watcher at all: whatever is captured, a pass captured.
const UNWATCHED: &[u8] = b"[triggers]\nactive = []\nrate_limit = \"1s\"\n";

fn write(root: &Path, path: &str, content: &[u8]) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, content).expect("a file");
}

async fn register(daemon: &Harness, project: &Path) -> String {
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    root["id"].as_str().expect("an id").to_owned()
}

async fn eventually<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(found) = probe().await {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "{what} did not happen within {PATIENCE:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The newest Snapshot that changed `path`, once one taken by `trigger` exists.
async fn snapshot_changing(daemon: &Harness, root: &str, path: &str, trigger: &str) -> Value {
    eventually(
        &format!("a `{trigger}` Snapshot changing `{path}`"),
        || async {
            let timeline = daemon
                .get_json(&format!(
                    "/v1/roots/{root}/snapshots?path={}",
                    keepster_api::encode_path_query(path)
                ))
                .await;
            timeline["snapshots"]
                .as_array()
                .and_then(|snapshots| snapshots.first())
                .filter(|newest| newest["trigger"] == trigger)
                .cloned()
        },
    )
    .await
}

async fn state_becomes(daemon: &Harness, root: &str, state: &str, code: Option<&str>) {
    eventually(&format!("the Root to be {state} ({code:?})"), || async {
        let shown = daemon.get_json(&format!("/v1/roots/{root}")).await;
        (shown["state"] == state && shown["state_code"].as_str() == code).then_some(())
    })
    .await;
}

fn project(daemon: &Harness) -> PathBuf {
    daemon.scratch("project")
}

#[tokio::test]
async fn what_changed_while_the_daemon_was_stopped_is_captured_when_it_starts() {
    let daemon = Harness::start_with_global_config(UNWATCHED).await;
    let project = project(&daemon);
    write(&project, "notes.txt", b"before");
    let root = register(&daemon, &project).await;
    snapshot_changing(&daemon, &root, "notes.txt", "reconcile").await;

    let stopped = daemon.stop().await;
    write(&project, "notes.txt", b"written while nothing ran");
    write(&project, "new.txt", b"created while nothing ran");
    let daemon = stopped.start().await;

    // The newest Snapshot changing it is the one before the stop until the start's pass lands.
    eventually("the start's pass to capture the edit", || async {
        let timeline = daemon
            .get_json(&format!("/v1/roots/{root}/snapshots?path=notes.txt"))
            .await;
        timeline["snapshots"]
            .as_array()
            .and_then(|snapshots| snapshots.first())
            .filter(|newest| newest["trigger"] == "reconcile" && newest["change"] == "modified")
            .cloned()
    })
    .await;
    snapshot_changing(&daemon, &root, "new.txt", "reconcile").await;
}

#[tokio::test]
async fn a_reload_that_changes_the_rules_reconciles_the_root() {
    let daemon = Harness::start_with_global_config(
        b"[triggers]\nactive = []\nrate_limit = \"1s\"\n[files]\nexclude = [\"*.log\"]\n",
    )
    .await;
    let project = project(&daemon);
    write(&project, "build.log", b"kept out, for now");
    write(&project, "main.rs", b"fn main() {}");
    let root = register(&daemon, &project).await;
    snapshot_changing(&daemon, &root, "main.rs", "reconcile").await;

    // Nothing in the Root changes, so no event could ever bring the file in — only the rules did.
    daemon.write_global_config(UNWATCHED);

    let taken = snapshot_changing(&daemon, &root, "build.log", "reconcile").await;
    assert_eq!(taken["change"], "added");
}

#[tokio::test]
async fn a_root_watched_and_reconciled_at_start_is_watching() {
    let daemon = Harness::start().await;
    let root = register(&daemon, &project(&daemon)).await;

    state_becomes(&daemon, &root, "watching", None).await;
}

#[tokio::test]
async fn a_root_whose_startup_walk_is_off_is_reported_as_not_guaranteed() {
    let daemon = Harness::start_with_global_config(b"[reconcile]\nat_start = false\n").await;
    let root = register(&daemon, &project(&daemon)).await;

    state_becomes(&daemon, &root, "degraded", Some("coverage_not_guaranteed")).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_root_past_the_watch_ceiling_is_reconciled_periodically_and_says_why() {
    // One watch for the whole daemon: the first Root's top directory takes it.
    let daemon = Harness::start_with_watch_ceiling(1).await;
    let first = register(&daemon, &daemon.scratch("first")).await;
    state_becomes(&daemon, &first, "watching", None).await;

    let second = register(&daemon, &daemon.scratch("second")).await;

    state_becomes(&daemon, &second, "degraded", Some("watch_ceiling")).await;
    state_becomes(&daemon, &first, "watching", None).await;
}

#[tokio::test]
async fn whole_second_mtimes_are_reported_until_hashing_is_chosen() {
    let daemon = Harness::start_with_global_config(UNWATCHED).await;
    let project = project(&daemon);
    write(
        &project,
        "copied.txt",
        b"from an archive that kept whole seconds",
    );
    std::fs::File::options()
        .write(true)
        .open(project.join("copied.txt"))
        .expect("the file")
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .expect("a whole-second mtime");
    let root = register(&daemon, &project).await;
    daemon.write_global_config(b"[triggers]\nactive = [\"watcher\"]\n");

    state_becomes(&daemon, &root, "degraded", Some("mtime_coarse")).await;

    daemon.write_global_config(b"[reconcile]\nverify = \"hash\"\n");
    state_becomes(&daemon, &root, "watching", None).await;
}

#[tokio::test]
async fn keepster_reconcile_runs_a_pass_now_and_verify_reads_everything() {
    let daemon = Harness::start_with_global_config(UNWATCHED).await;
    let project = project(&daemon);
    write(&project, "a.txt", b"one");
    write(&project, "b.txt", b"two");
    let root = register(&daemon, &project).await;
    snapshot_changing(&daemon, &root, "a.txt", "reconcile").await;

    let quiet = daemon
        .run_cli(KEEPSTER, &["reconcile", "--root", &root, "--json"])
        .await;
    assert_eq!(quiet.exit_code, Some(0), "{}", quiet.stderr);
    let quiet: Value = serde_json::from_slice(&quiet.stdout).expect("JSON");
    assert_eq!(quiet["observed"], 2);
    assert!(
        quiet.get("snapshot").is_none(),
        "nothing had changed: {quiet}"
    );

    let verified = daemon
        .run_cli(
            KEEPSTER,
            &["reconcile", "--root", &root, "--verify", "--json"],
        )
        .await;
    let verified: Value = serde_json::from_slice(&verified.stdout).expect("JSON");
    assert_eq!(verified["hashed"], 2, "every file is read");

    write(&project, "a.txt", b"edited");
    let printed = daemon.run_cli_in(&project, KEEPSTER, &["reconcile"]).await;
    assert_eq!(printed.exit_code, Some(0), "{}", printed.stderr);
    let text = String::from_utf8(printed.stdout).expect("text");
    assert!(text.contains("snapshot"), "{text}");
}
