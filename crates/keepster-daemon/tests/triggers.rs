//! Automatic capture at the `/v1` seam (ticket 43): an edit becomes a Snapshot by itself, an atomic save
//! is an edit, and a stopping daemon flushes what is still pending.
//!
//! Every assertion waits for something to happen — never for something not to: the watcher's events
//! arrive late, coalesced or not at all, and a test that waits for silence proves only its timeout.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use keepster_testkit::Harness;
use serde_json::Value;

/// Long enough for a loaded CI runner; each test finishes as soon as what it waits for happens.
const PATIENCE: Duration = Duration::from_secs(30);

/// A quick debounce and rate limit, so a test waits for a Snapshot in moments rather than a minute.
const QUICK: &[u8] = b"[triggers]\ndebounce = \"200ms\"\nrate_limit = \"1s\"\n";

fn write(root: &Path, path: &str, content: &[u8]) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, content).expect("a file");
}

/// A registered Root, once the Reconciliation its registration owes has written its first Snapshot —
/// so that what a test writes next is the watcher's to capture, not that pass's.
async fn registered(daemon: &Harness) -> (String, PathBuf) {
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let root = root["id"].as_str().expect("an id").to_owned();
    eventually("the registration's Reconciliation", || async {
        let timeline = daemon
            .get_json(&format!("/v1/roots/{root}/snapshots"))
            .await;
        timeline["snapshots"]
            .as_array()
            .filter(|snapshots| {
                snapshots
                    .iter()
                    .any(|taken| taken["trigger"] == "reconcile")
            })
            .map(|_| ())
    })
    .await;

    (root, project)
}

/// Poll `probe` until it yields something, failing the test after [`PATIENCE`].
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
                .and_then(|items| items.first())
                .filter(|newest| newest["trigger"] == trigger)
                .cloned()
        },
    )
    .await
}

#[tokio::test]
async fn an_edit_becomes_a_snapshot_by_itself() {
    let daemon = Harness::start_with_global_config(QUICK).await;
    let (root, project) = registered(&daemon).await;

    write(&project, "notes.txt", b"written while watched");

    let taken = snapshot_changing(&daemon, &root, "notes.txt", "watcher").await;
    assert_eq!(taken["change"], "added");
}

#[tokio::test]
async fn an_atomic_save_is_captured_as_an_edit() {
    let daemon = Harness::start_with_global_config(QUICK).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"first");
    let first = daemon
        .post_json(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await;
    let first = first["id"].as_str().expect("an id").to_owned();

    // What an editor does: write the new contents beside the file, then rename over it.
    write(&project, ".notes.txt.tmp", b"second");
    std::fs::rename(project.join(".notes.txt.tmp"), project.join("notes.txt")).expect("the save");

    // The watcher may have captured the first write on its own before the Snapshot above was taken —
    // as an addition — so only a Snapshot after that one is the save's.
    let taken = eventually("a `watcher` Snapshot of the save", || async {
        let timeline = daemon
            .get_json(&format!("/v1/roots/{root}/snapshots?path=notes.txt"))
            .await;
        timeline["snapshots"]
            .as_array()
            .and_then(|items| items.first())
            .filter(|newest| {
                newest["trigger"] == "watcher"
                    && newest["id"].as_str().is_some_and(|id| id > first.as_str())
            })
            .cloned()
    })
    .await;
    assert_eq!(taken["change"], "modified");
}

#[tokio::test]
async fn the_periodic_floor_captures_when_it_is_switched_on() {
    let daemon = Harness::start_with_global_config(
        b"[triggers]\nactive = [\"periodic\"]\nperiodic = \"1s\"\nrate_limit = \"1s\"\n",
    )
    .await;
    let (root, project) = registered(&daemon).await;

    // No watcher: only the floor can find this.
    write(&project, "unwatched.txt", b"written with nothing watching");

    snapshot_changing(&daemon, &root, "unwatched.txt", "periodic").await;
}

#[tokio::test]
async fn stopping_flushes_a_debounce_window_still_counting_down() {
    // A window that would not close for an hour: only the stop can capture this edit.
    let daemon = Harness::start_with_global_config(
        b"[triggers]\ndebounce = \"1h\"\nrate_limit = \"1s\"\n[reconcile]\nat_start = false\n",
    )
    .await;
    let (root, project) = registered(&daemon).await;
    write(&project, "unsaved-work.txt", b"the most recent edit");
    eventually("the edit to be pending", || async {
        let shown = daemon.get_json(&format!("/v1/roots/{root}")).await;
        (shown["pending_paths"].as_u64() > Some(0)).then_some(())
    })
    .await;

    let daemon = daemon.restart().await;

    snapshot_changing(&daemon, &root, "unsaved-work.txt", "watcher").await;
}

#[tokio::test]
async fn a_root_with_nothing_waiting_reports_no_pending_change() {
    let daemon = Harness::start_with_global_config(QUICK).await;
    let (root, _) = registered(&daemon).await;

    // Whatever the platform reported while the watch was armed is captured within moments; after that,
    // nothing waits.
    let shown = eventually("nothing to be pending", || async {
        let shown = daemon.get_json(&format!("/v1/roots/{root}")).await;
        (shown["pending_paths"] == 0).then_some(shown)
    })
    .await;
    assert!(shown.get("oldest_pending_seconds").is_none(), "{shown}");
}

#[tokio::test]
async fn the_trigger_settings_are_reported_with_their_defaults() {
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;

    let configuration = daemon.get_json(&format!("/v1/roots/{root}/config")).await;
    let value = |name: &str| {
        configuration["keys"]
            .as_array()
            .expect("the keys")
            .iter()
            .find(|key| key["key"] == name)
            .map(|key| key["value"].clone())
            .unwrap_or_else(|| panic!("`{name}` is reported"))
    };

    assert_eq!(value("triggers.active"), serde_json::json!(["watcher"]));
    assert_eq!(value("triggers.debounce"), "3s");
    assert_eq!(value("triggers.rate_limit"), "1m");
    assert_eq!(value("triggers.periodic"), "1h");
}

#[cfg(unix)]
#[tokio::test]
async fn a_chmod_alone_becomes_a_snapshot_by_itself() {
    use std::os::unix::fs::PermissionsExt;

    let daemon = Harness::start_with_global_config(QUICK).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "key", b"secret enough");
    snapshot_changing(&daemon, &root, "key", "watcher").await;

    std::fs::set_permissions(project.join("key"), std::fs::Permissions::from_mode(0o600))
        .expect("its mode");

    // The watcher passes an attribute event on, and the pass reads the mode from its `stat`.
    let taken = eventually("a `watcher` Snapshot for the mode alone", || async {
        let timeline = daemon
            .get_json(&format!("/v1/roots/{root}/snapshots?path=key"))
            .await;
        timeline["snapshots"]
            .as_array()
            .and_then(|items| items.first())
            .filter(|newest| newest["trigger"] == "watcher" && newest["change"] == "modified")
            .cloned()
    })
    .await;
    let id = taken["id"].as_str().expect("an id");
    let entries = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots/{id}/entries"))
        .await;
    assert_eq!(entries["entries"][0]["permissions"], 0o600, "{entries}");
}
