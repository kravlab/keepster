//! Retention in the daemon (ticket 51): a pass is owed once per Segment rollover — and so at the start —
//! runs at the first quiet moment under the store's lock, and announces what it removed.

use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, RootId, Trigger};
use keepster_daemon::RetentionTiming;
use keepster_store::HistoryStore;
use keepster_testkit::Harness;
use serde_json::{Value, json};

const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

/// A look every second, a second of quiet, and an hour's ceiling the test never reaches.
const QUICK: RetentionTiming = RetentionTiming {
    every: Duration::from_secs(1),
    quiet: Duration::from_secs(1),
    ceiling: Duration::from_secs(3600),
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
    let daemon = Harness::start_with_retention(QUICK, keepster_testkit::MANUAL).await;
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
    let completed = loop {
        let event = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("a Retention pass within the timeout")
            .expect("the stream reads")
            .expect("open");
        if event.event == "retention.completed" {
            break serde_json::from_str::<Value>(&event.data).expect("JSON");
        }
    };

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
