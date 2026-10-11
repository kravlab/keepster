//! Housekeeping in the daemon (tickets 51 and 52): Retention is owed once per Segment rollover — and so
//! at the start — runs at the first quiet moment under the store's lock, and announces what it removed;
//! the Maintenance Pass that follows packs the store with the `git` `status` names.

use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, RootId, Trigger};
use keepster_daemon::HousekeepingTiming;
use keepster_store::HistoryStore;
use keepster_testkit::Harness;
use serde_json::{Value, json};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

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

#[tokio::test]
async fn the_start_schedules_a_pass_that_thins_old_history_and_says_so() {
    let daemon = Harness::start_with_housekeeping(QUICK, keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.txt"), b"one").expect("a file");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let directory = daemon.store_dir(&root);
    let stopped = daemon.stop().await;

    // Ten days ago, three hours of one day with three Snapshots each — written while the daemon was
    // down, as a month of history would have been — and one an hour ago.
    let location = keepster_app::snapshots::location(&directory);
    let store = HistoryStore::open(&location.store, &location.index).expect("the store");
    let id: RootId = root.parse().expect("a Root id");
    let day = (now() - 10 * DAY) / DAY * DAY;
    let mut moments = Vec::new();
    for hour in [3, 9, 20] {
        for minute in [5, 25, 45] {
            moments.push(day + hour * HOUR + minute * 60);
        }
    }
    moments.push(now() - HOUR);
    for (count, moment) in moments.iter().enumerate() {
        std::fs::write(project.join("notes.txt"), format!("version {count}")).expect("a change");
        store
            .capture(
                id,
                &project,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(*moment),
            )
            .expect("a Snapshot");
    }
    drop(store);

    let daemon = stopped.start().await;
    let mut stream = daemon.events(None).await;
    let mut completed = None;
    let mut maintained = None;
    while completed.is_none() || maintained.is_none() {
        let event = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("Retention and a Maintenance Pass within the timeout")
            .expect("the stream reads")
            .expect("open");
        let data = || serde_json::from_str::<Value>(&event.data).expect("JSON");
        match event.event.as_str() {
            "retention.completed" => {
                assert!(
                    maintained.is_none(),
                    "Retention condemns first, then the pass reclaims"
                );
                completed = Some(data());
            }
            "maintenance.completed" => maintained = Some(data()),
            _ => {}
        }
    }
    let completed = completed.expect("Retention");
    assert_eq!(
        maintained,
        Some(json!({ "event": "maintenance.completed", "root": root }))
    );
    assert!(
        std::fs::read_dir(location.store.join("objects/pack"))
            .expect("packs")
            .count()
            > 0,
        "the pass packed what survived"
    );

    assert_eq!(
        completed,
        json!({
            "event": "retention.completed",
            "root": root,
            "deleted_segments": 2,
            "thinned_segments": 1,
            "removed_snapshots": 8,
        }),
        "the day keeps its last Snapshot, in the last of its hours"
    );
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(
        timeline["snapshots"].as_array().map(Vec::len),
        Some(2),
        "{timeline}"
    );

    // The store records its pass, so a restarted daemon does not owe another for a day.
    let daemon = daemon.restart().await;
    let mut stream = daemon.events(None).await;
    let quiet_looks = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = stream
                .next()
                .await
                .expect("the stream reads")
                .expect("open");
            if event.event == "maintenance.completed" {
                break event;
            }
        }
    })
    .await;
    assert!(quiet_looks.is_err(), "maintained again: {quiet_looks:?}");
}

#[tokio::test]
async fn the_ladder_is_part_of_the_configuration() {
    let daemon = Harness::start_with_global_config(
        b"[triggers]\nactive = []\n\n[[retention.tier]]\nage = \"2d\"\nkeep = \"all\"\n",
    )
    .await;
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let config = daemon.get_json(&format!("/v1/roots/{root}/config")).await;
    let ladder = config["keys"]
        .as_array()
        .expect("keys")
        .iter()
        .find(|key| key["key"] == "retention.tier")
        .cloned()
        .expect("the ladder");

    assert_eq!(
        ladder["value"],
        json!([{ "age": "2d", "keep": "all" }]),
        "{ladder}"
    );
    assert_eq!(ladder["origin"]["level"], "global", "{ladder}");
}

#[tokio::test]
async fn status_names_the_git_maintenance_runs_with() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;

    let status = daemon.get_json("/v1/status").await;

    assert_eq!(status["daemon"]["maintenance"], "available", "{status}");
    let git = &status["daemon"]["git"];
    assert!(
        git["path"].as_str().is_some_and(|path| !path.is_empty()),
        "{git}"
    );
    assert!(git["version"].is_string(), "{git}");
    assert_eq!(git["minimum"], "2.32.0");
    assert!(git.get("reason").is_none(), "{git}");
}

#[tokio::test]
async fn a_configured_git_that_does_not_work_is_reported_and_never_replaced() {
    let daemon = Harness::start_with_housekeeping(QUICK, keepster_testkit::MANUAL).await;
    let missing = daemon.scratch("nowhere").join("git");
    daemon.write_global_config(
        format!(
            "[triggers]\nactive = []\n\n[git]\npath = {}\n",
            toml_string(&missing.display().to_string())
        )
        .as_bytes(),
    );

    // Found again at the next housekeeping look, a second away.
    let status = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let status = daemon.get_json("/v1/status").await;
            if status["daemon"]["maintenance"] == "unavailable" {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the configured git is looked at");

    let git = &status["daemon"]["git"];
    assert_eq!(git["reason"], "not_executable", "{git}");
    assert_eq!(git["path"], missing.display().to_string(), "{git}");
    assert!(git["install"]["link"].is_string(), "{git}");

    let run = daemon.run_cli(KEEPSTER, &["status"]).await;
    assert_eq!(run.exit_code, Some(11), "{}", run.stderr);
    let printed = String::from_utf8_lossy(&run.stdout);
    assert!(printed.contains("maintenance  unavailable"), "{printed}");

    // Watching is never refused for it, but it says so.
    let project = daemon.scratch("project");
    let run = daemon
        .run_cli(KEEPSTER, &["watch", &project.display().to_string()])
        .await;
    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    assert!(
        run.stderr.contains("store maintenance is unavailable"),
        "{}",
        run.stderr
    );
}

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

/// `text` as a TOML basic string, so a Windows path's backslashes survive.
fn toml_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\"))
}
