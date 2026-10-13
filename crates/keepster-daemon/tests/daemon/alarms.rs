//! Alarms, the mute and the trays' reports at the `/v1` seam (ticket 67): the daemon decides what stopped
//! the insurance and says so on the stream and in `status`, so that a tray only ever delivers.

use std::time::Duration;

use keepster_testkit::Harness;
use serde_json::{Value, json};

use crate::events::{next, next_named};

fn json(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

/// The daemon's rows of `status`.
async fn daemon_row(daemon: &Harness) -> Value {
    daemon.get_json("/v1/status").await["daemon"].clone()
}

/// The alarms of `kind` among `alarms`.
fn of_kind<'a>(alarms: &'a Value, kind: &str) -> Vec<&'a Value> {
    alarms
        .as_array()
        .expect("a list of alarms")
        .iter()
        .filter(|alarm| alarm["kind"] == kind)
        .collect()
}

#[tokio::test]
async fn a_root_that_stops_being_watched_raises_an_alarm_that_status_lists_and_recovery_clears() {
    // Captured automatically, so that the Root is `watching` once its first Reconciliation has run.
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    tokio::time::timeout(Duration::from_secs(30), async {
        while daemon.get_json(&format!("/v1/roots/{root}")).await["state"] != "watching" {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the Root is watched");
    let mut stream = daemon.events(None).await;

    std::fs::remove_dir_all(&project).expect("the directory goes away");

    let (_, raised) = next_named(&mut stream, "alarms.changed").await;
    let alarm = json!({ "kind": "root_unavailable", "root": root, "reason": "root_missing" });
    assert_eq!(
        raised,
        json!({ "event": "alarms.changed", "alarms": [alarm], "raised": [alarm], "cleared": [] })
    );
    assert_eq!(daemon_row(&daemon).await["alarms"], json!([alarm]));

    std::fs::create_dir(&project).expect("the directory comes back");

    // On its way back the Root may be `degraded` until it is reconciled again, which is an alarm of its
    // own if it lasts: what matters is that the `unavailable` one is cleared, and that nothing is left.
    let mut cleared = Vec::new();
    loop {
        let (_, changed) = next_named(&mut stream, "alarms.changed").await;
        cleared.extend(changed["cleared"].as_array().expect("a list").clone());
        if changed["alarms"] == json!([]) {
            break;
        }
    }
    assert!(cleared.contains(&alarm), "{cleared:?}");
    assert_eq!(daemon_row(&daemon).await["alarms"], json!([]));
}

#[tokio::test]
async fn a_job_that_fails_raises_an_alarm_naming_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let root = crate::jobs::captured(&daemon).await;
    std::fs::write(
        daemon.store_dir(&root).join("history.git").join("config"),
        b"[core\n",
    )
    .expect("the store's configuration, broken");
    let mut stream = daemon.events(None).await;

    let job = json(
        &daemon
            .post(&format!("/v1/roots/{root}/maintenance"), &json!({}))
            .await,
    )["job_id"]
        .as_str()
        .expect("a job id")
        .to_owned();

    let raised = loop {
        let (_, changed) = next_named(&mut stream, "alarms.changed").await;
        if !of_kind(&changed["raised"], "job_failed").is_empty() {
            break changed;
        }
    };
    assert_eq!(
        of_kind(&raised["raised"], "job_failed"),
        [&json!({ "kind": "job_failed", "root": root, "job": job, "job_kind": "maintenance" })]
    );
    assert_eq!(
        of_kind(&daemon_row(&daemon).await["alarms"], "job_failed").len(),
        1
    );
}

#[tokio::test]
async fn a_mute_is_shown_in_status_and_announced_until_it_is_lifted() {
    let daemon = Harness::start().await;
    let mut stream = daemon.events(None).await;

    let muted = daemon.put_json("/v1/alarms/mute", &json!({})).await;

    assert_eq!(
        muted.status,
        200,
        "{}",
        String::from_utf8_lossy(&muted.body)
    );
    let until = json(&muted)["until"]
        .as_str()
        .expect("when it ends")
        .to_owned();
    let lasts = until
        .parse::<jiff::Timestamp>()
        .expect("an RFC 3339 moment")
        .duration_since(jiff::Timestamp::now());
    assert!(
        (jiff::SignedDuration::from_mins(59)..=jiff::SignedDuration::from_mins(60))
            .contains(&lasts),
        "an hour unless told otherwise: {lasts:?}"
    );
    assert_eq!(daemon_row(&daemon).await["muted_until"], until.as_str());
    let (_, announced) = next_named(&mut stream, "alarms.changed").await;
    assert_eq!(
        announced,
        json!({ "event": "alarms.changed", "alarms": [], "raised": [], "cleared": [], "muted_until": until })
    );

    let lifted = daemon.delete("/v1/alarms/mute").await;

    assert_eq!(lifted.status, 200);
    assert_eq!(json(&lifted), json!({}));
    assert!(daemon_row(&daemon).await.get("muted_until").is_none());
    let (_, announced) = next_named(&mut stream, "alarms.changed").await;
    assert!(announced.get("muted_until").is_none(), "{announced}");
}

#[tokio::test]
async fn a_mute_for_longer_than_a_day_or_shorter_than_a_minute_is_refused() {
    let daemon = Harness::start().await;

    for minutes in [0, -1, 1_441] {
        let refused = daemon
            .put_json("/v1/alarms/mute", &json!({ "minutes": minutes }))
            .await;

        assert_eq!(refused.status, 400, "{minutes}");
        assert_eq!(json(&refused)["code"], "mute_invalid", "{minutes}");
    }
    assert!(daemon_row(&daemon).await.get("muted_until").is_none());
}

#[tokio::test]
async fn a_trays_report_is_listed_in_status_until_it_withdraws() {
    let daemon = Harness::start().await;
    assert_eq!(
        daemon_row(&daemon).await["trays"],
        json!([]),
        "no tray attached"
    );
    let report = json!({
        "icon": "available",
        "notifications": "unavailable",
        "notifications_reason": "no_server",
    });

    let reported = daemon.put_json("/v1/trays/tray-1", &report).await;

    assert_eq!(
        reported.status,
        204,
        "{}",
        String::from_utf8_lossy(&reported.body)
    );
    assert_eq!(
        daemon_row(&daemon).await["trays"],
        json!([{
            "instance": "tray-1",
            "icon": "available",
            "notifications": "unavailable",
            "notifications_reason": "no_server",
        }])
    );

    let withdrawn = daemon.delete("/v1/trays/tray-1").await;

    assert_eq!(withdrawn.status, 204);
    assert_eq!(daemon_row(&daemon).await["trays"], json!([]));
}

#[tokio::test]
async fn a_tray_instance_that_is_not_a_plain_name_is_refused() {
    let daemon = Harness::start().await;
    let report = json!({ "icon": "available", "notifications": "available" });

    let refused = daemon.put_json("/v1/trays/a%20b", &report).await;

    assert_eq!(refused.status, 400);
    assert_eq!(json(&refused)["code"], "tray_invalid");
    assert_eq!(daemon_row(&daemon).await["trays"], json!([]));
}

#[tokio::test]
async fn a_daemon_that_stops_says_so_before_its_stream_ends() {
    let daemon = Harness::start().await;
    let mut stream = daemon.events(None).await;

    let stopped = tokio::spawn(daemon.stop());

    assert_eq!(next(&mut stream).await.event, "daemon.stopping");
    let ended = tokio::time::timeout(Duration::from_secs(30), stream.next())
        .await
        .expect("the stream ends")
        .expect("the stream reads");
    assert!(ended.is_none(), "nothing follows: {ended:?}");
    stopped.await.expect("the daemon stops");
}

#[tokio::test]
async fn a_raise_of_the_log_level_is_announced_and_so_is_its_end() {
    let daemon = Harness::start().await;
    let mut stream = daemon.events(None).await;

    let raised = daemon
        .put_json("/v1/log/level", &json!({ "level": "debug", "for": "30m" }))
        .await;

    let until = json(&raised)["until"].clone();
    let (_, announced) = next_named(&mut stream, "log.level_changed").await;
    assert_eq!(
        announced,
        json!({ "event": "log.level_changed", "level": "debug", "until": until })
    );

    daemon.delete("/v1/log/level").await;

    let (_, announced) = next_named(&mut stream, "log.level_changed").await;
    assert_eq!(
        announced,
        json!({ "event": "log.level_changed", "level": "info" })
    );
}
