//! Pins through `/v1` (ticket 53): pinning and unpinning are synchronous and idempotent, a Root no
//! longer watched has its newest Snapshot pinned and its history decaying down the ladder to it, and
//! watching its directory again finds that history.

use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, RootId, Trigger};
use keepster_daemon::HousekeepingTiming;
use keepster_store::HistoryStore;
use keepster_testkit::Harness;
use serde_json::{Value, json};

const DAY: u64 = 24 * 3600;

/// A look every second, a second of quiet, and an hour's ceiling the test never reaches.
const QUICK: HousekeepingTiming = HousekeepingTiming {
    every: Duration::from_secs(1),
    quiet: Duration::from_secs(1),
    ceiling: Duration::from_secs(3600),
    maintain_every: Duration::from_secs(24 * 3600),
};

/// The daemon's own clock, which the history written behind its back has to agree with.
#[expect(
    clippy::disallowed_methods,
    reason = "the test stands in for the daemon, the one place ADR 0012 lets read the clock"
)]
fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs()
}

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// Register a directory holding one file, and return its identity.
async fn watched(daemon: &Harness) -> (String, std::path::PathBuf) {
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.txt"), b"one").expect("a file");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    (root, project)
}

async fn snapshot(
    daemon: &Harness,
    root: &str,
    content: &str,
    project: &std::path::Path,
) -> String {
    std::fs::write(project.join("notes.txt"), content).expect("a change");
    daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await["id"]
        .as_str()
        .expect("a Snapshot")
        .to_owned()
}

fn text(response: &keepster_cli::Response) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

fn json(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

fn ids(pins: &Value) -> Vec<&str> {
    pins["snapshots"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|snapshot| snapshot["id"].as_str().expect("an id"))
        .collect()
}

#[tokio::test]
async fn pinning_and_unpinning_are_idempotent_and_listed() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = watched(&daemon).await;
    let first = snapshot(&daemon, &root, "first", &project).await;
    let second = snapshot(&daemon, &root, "second", &project).await;
    let pin = format!("/v1/roots/{root}/pins/{first}");

    for _ in 0..2 {
        let answer = daemon.put(&pin).await;
        assert_eq!(answer.status.as_u16(), 204, "{}", text(&answer));
    }

    let pins = daemon.get_json(&format!("/v1/roots/{root}/pins")).await;
    assert_eq!(ids(&pins), [first.as_str()], "{pins}");
    assert_eq!(pins["snapshots"][0]["pinned"], true);
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    let pinned: Vec<(&str, bool)> = timeline["snapshots"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|snapshot| {
            (
                snapshot["id"].as_str().expect("an id"),
                snapshot["pinned"].as_bool().expect("a flag"),
            )
        })
        .collect();
    assert_eq!(
        pinned,
        [(second.as_str(), false), (first.as_str(), true)],
        "{timeline}"
    );

    for _ in 0..2 {
        let answer = daemon.delete(&pin).await;
        assert_eq!(answer.status.as_u16(), 204, "{}", text(&answer));
    }
    let pins = daemon.get_json(&format!("/v1/roots/{root}/pins")).await;
    assert!(ids(&pins).is_empty(), "{pins}");

    let unknown = daemon
        .put(&format!("/v1/roots/{root}/pins/01K6DX3QR7M2V8C9WZ4PN5YQ1A"))
        .await;
    assert_eq!(unknown.status.as_u16(), 404);
    assert_eq!(
        json(&unknown)["code"],
        "snapshot_not_found",
        "{}",
        text(&unknown)
    );
}

#[tokio::test]
async fn unwatching_pins_the_last_state_and_watching_again_finds_the_history() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = watched(&daemon).await;
    let earlier = snapshot(&daemon, &root, "earlier", &project).await;
    let last = snapshot(&daemon, &root, "last", &project).await;

    let answer = daemon.delete(&format!("/v1/roots/{root}")).await;
    assert_eq!(answer.status.as_u16(), 204, "{}", text(&answer));

    // No longer watched, the history is still there to read, with its last state pinned.
    assert_eq!(
        daemon
            .get(&format!("/v1/roots/{root}"))
            .await
            .status
            .as_u16(),
        404,
        "not a watched Root"
    );
    let pins = daemon.get_json(&format!("/v1/roots/{root}/pins")).await;
    assert_eq!(ids(&pins), [last.as_str()], "{pins}");
    let roots = daemon.get_json("/v1/roots").await;
    assert_eq!(roots.as_array().map(Vec::len), Some(0), "{roots}");

    // An ordinary Pin: it can be unpinned like any other, and pinned again.
    let pin = format!("/v1/roots/{root}/pins/{last}");
    assert_eq!(daemon.delete(&pin).await.status.as_u16(), 204);
    assert_eq!(daemon.put(&pin).await.status.as_u16(), 204);

    let again = daemon.post("/v1/roots", &json!({ "path": project })).await;
    assert_eq!(again.status.as_u16(), 201, "{}", text(&again));
    assert_eq!(
        json(&again)["id"],
        root.as_str(),
        "the same Root, not a new one"
    );
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    let held: Vec<&str> = timeline["snapshots"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|snapshot| snapshot["id"].as_str().expect("an id"))
        .collect();
    assert!(
        held.contains(&earlier.as_str()) && held.contains(&last.as_str()),
        "the history it had: {timeline}"
    );
}

#[tokio::test]
async fn a_root_that_never_captured_anything_has_nothing_to_pin() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("empty");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let answer = daemon.delete(&format!("/v1/roots/{root}")).await;

    assert_eq!(answer.status.as_u16(), 204, "{}", text(&answer));
    let pins = daemon.get_json(&format!("/v1/roots/{root}/pins")).await;
    assert!(ids(&pins).is_empty(), "{pins}");
}

#[tokio::test]
async fn a_root_no_longer_watched_decays_down_the_ladder_to_its_pinned_last_state() {
    let daemon = Harness::start_with_housekeeping(QUICK, keepster_testkit::MANUAL).await;
    let (root, project) = watched(&daemon).await;
    let directory = daemon.store_dir(&root);
    let stopped = daemon.stop().await;

    // A month and more of history, written while the daemon was down; the last of it a month old too.
    let location = keepster_app::snapshots::location(&directory);
    let store = HistoryStore::open(&location.store, &location.index).expect("the store");
    let id: RootId = root.parse().expect("a Root id");
    for (count, age) in [45, 40, 35, 32].into_iter().enumerate() {
        std::fs::write(project.join("notes.txt"), format!("version {count}")).expect("a change");
        store
            .capture(
                id,
                &project,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(now() - age * DAY),
            )
            .expect("a Snapshot");
    }
    drop(store);

    let daemon = stopped.start().await;
    let mut stream = daemon.events(None).await;
    assert_eq!(
        daemon
            .delete(&format!("/v1/roots/{root}"))
            .await
            .status
            .as_u16(),
        204
    );

    // Retention goes on over the history kept: what the ladder no longer keeps goes.
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("Retention over the unwatched Root within the timeout")
            .expect("the stream reads")
            .expect("open");
        if event.event == "retention.completed" {
            let data: Value = serde_json::from_str(&event.data).expect("JSON");
            assert_eq!(data["root"], root.as_str(), "{data}");
            break;
        }
    }
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    let left: Vec<(&str, bool)> = timeline["snapshots"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|snapshot| {
            (
                snapshot["id"].as_str().expect("an id"),
                snapshot["pinned"].as_bool().expect("a flag"),
            )
        })
        .collect();
    let pins = daemon.get_json(&format!("/v1/roots/{root}/pins")).await;
    let last = ids(&pins);
    assert_eq!(last.len(), 1, "{pins}");
    assert_eq!(
        left,
        [(last[0], true)],
        "only the pinned last state remains: {timeline}"
    );

    // Unpinned by the user's own act, nothing holds it any more.
    let unpinned = daemon
        .delete(&format!("/v1/roots/{root}/pins/{}", last[0]))
        .await;
    assert_eq!(unpinned.status.as_u16(), 204);
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(
        timeline["snapshots"].as_array().map(Vec::len),
        Some(0),
        "{timeline}"
    );
}

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn the_cli_pins_lists_and_unpins_and_reaches_a_root_no_longer_watched() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = watched(&daemon).await;
    let first = snapshot(&daemon, &root, "first", &project).await;
    let second = snapshot(&daemon, &root, "second", &project).await;

    // Standing in the Root, `latest` is the default.
    let pinned = daemon.run_cli_in(&project, KEEPSTER, &["pin"]).await;
    assert_eq!(pinned.exit_code, Some(0), "{}", pinned.stderr);
    assert!(
        String::from_utf8_lossy(&pinned.stdout).contains(&second),
        "it says which: {}",
        String::from_utf8_lossy(&pinned.stdout)
    );
    let pinned = daemon.run_cli(KEEPSTER, &["pin", "-r", &root, "~1"]).await;
    assert_eq!(pinned.exit_code, Some(0), "{}", pinned.stderr);

    let listed = daemon
        .run_cli(KEEPSTER, &["pins", "-r", &root, "--json"])
        .await;
    assert_eq!(listed.exit_code, Some(0), "{}", listed.stderr);
    let listed: Value = serde_json::from_slice(&listed.stdout).expect("JSON");
    let listed: Vec<&str> = listed
        .as_array()
        .expect("a list")
        .iter()
        .map(|snapshot| snapshot["id"].as_str().expect("an id"))
        .collect();
    assert_eq!(listed, [second.as_str(), first.as_str()]);

    let unpinned = daemon
        .run_cli(KEEPSTER, &["unpin", "-r", &root, &first])
        .await;
    assert_eq!(unpinned.exit_code, Some(0), "{}", unpinned.stderr);

    let unknown = daemon
        .run_cli(
            KEEPSTER,
            &["pin", "-r", &root, "01K6DX3QR7M2V8C9WZ4PN5YQ1A"],
        )
        .await;
    assert_eq!(unknown.exit_code, Some(4), "{}", unknown.stderr);

    // Unwatched, the Root is still reached by its full identity, and says how.
    let unwatched = daemon.run_cli_in(&project, KEEPSTER, &["unwatch"]).await;
    assert_eq!(unwatched.exit_code, Some(0), "{}", unwatched.stderr);
    let said = String::from_utf8_lossy(&unwatched.stdout);
    assert!(said.contains(&format!("keepster pins -r {root}")), "{said}");
    let listed = daemon.run_cli(KEEPSTER, &["pins", "-r", &root]).await;
    assert_eq!(listed.exit_code, Some(0), "{}", listed.stderr);
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains(&second[..10]),
        "{}",
        String::from_utf8_lossy(&listed.stdout)
    );
    let unpinned = daemon
        .run_cli(KEEPSTER, &["unpin", "-r", &root, "latest"])
        .await;
    assert_eq!(unpinned.exit_code, Some(0), "{}", unpinned.stderr);
    let pins = daemon.get_json(&format!("/v1/roots/{root}/pins")).await;
    assert!(ids(&pins).is_empty(), "{pins}");
}

#[tokio::test]
async fn the_history_of_a_root_no_longer_watched_can_still_be_purged() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = watched(&daemon).await;
    snapshot(&daemon, &root, "kept", &project).await;
    assert_eq!(
        daemon
            .delete(&format!("/v1/roots/{root}"))
            .await
            .status
            .as_u16(),
        204
    );

    let purged = daemon.delete(&format!("/v1/roots/{root}?purge=true")).await;

    assert_eq!(purged.status.as_u16(), 204, "{}", text(&purged));
    assert!(!daemon.store_dir(&root).exists(), "erased");
    assert_eq!(
        daemon
            .get(&format!("/v1/roots/{root}/pins"))
            .await
            .status
            .as_u16(),
        404
    );
    assert_eq!(
        daemon
            .delete(&format!("/v1/roots/{root}"))
            .await
            .status
            .as_u16(),
        404,
        "without `purge`, a Root no longer watched is not one to unwatch"
    );
}
