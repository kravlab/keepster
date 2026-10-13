//! The live event stream at the `/v1` seam (ticket 45): what happens, as it happens, over the socket,
//! with a reconnecting client replayed what it missed — or told honestly that it cannot be.

use std::path::Path;
use std::time::Duration;

use keepster_cli::{EventStream, SseEvent};
use keepster_testkit::Harness;
use serde_json::Value;

/// Long enough for a loaded CI runner; each test finishes as soon as what it waits for arrives.
const PATIENCE: Duration = Duration::from_secs(30);

/// The next event the stream delivers, failing the test if none arrives.
pub(crate) async fn next(stream: &mut EventStream) -> SseEvent {
    tokio::time::timeout(PATIENCE, stream.next())
        .await
        .unwrap_or_else(|_| panic!("no event within {PATIENCE:?}"))
        .expect("the stream reads")
        .expect("the stream stays open")
}

/// The next event named `name`, passing over the others, with its payload parsed.
pub(crate) async fn next_named(stream: &mut EventStream, name: &str) -> (SseEvent, Value) {
    loop {
        let event = next(stream).await;
        if event.event == name {
            let payload = serde_json::from_str(&event.data).expect("JSON data");
            return (event, payload);
        }
    }
}

async fn register(daemon: &Harness, project: &Path) -> String {
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    root["id"].as_str().expect("an id").to_owned()
}

async fn take(daemon: &Harness, root: &str) -> Value {
    daemon
        .post_json(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await
}

#[tokio::test]
async fn a_snapshot_is_announced_with_its_identity_and_flags_but_no_content() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    std::fs::write(project.join("secret.txt"), "never on the stream").expect("a file");
    let root = register(&daemon, &project).await;
    let mut stream = daemon.events(None).await;

    let taken = take(&daemon, &root).await;

    let (event, payload) = next_named(&mut stream, "snapshot.created").await;
    assert!(event.id.is_some(), "every event can be resumed after");
    assert_eq!(
        payload,
        serde_json::json!({
            "event": "snapshot.created",
            "root": root,
            "snapshot": taken["id"],
            "incomplete": false,
            "changed_paths": 1,
        })
    );
}

#[tokio::test]
async fn a_client_that_reconnects_with_the_last_id_it_saw_is_replayed_what_followed() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let root = register(&daemon, &project).await;
    let mut stream = daemon.events(None).await;
    let mut taken = Vec::new();
    for content in ["one", "two", "three"] {
        std::fs::write(project.join("notes.txt"), content).expect("an edit");
        taken.push(take(&daemon, &root).await["id"].clone());
    }
    let (seen, _) = next_named(&mut stream, "snapshot.created").await;
    drop(stream);

    let mut resumed = daemon.events(seen.id.as_deref()).await;

    for expected in &taken[1..] {
        let (_, payload) = next_named(&mut resumed, "snapshot.created").await;
        assert_eq!(&payload["snapshot"], expected);
    }
}

#[tokio::test]
async fn a_client_whose_last_id_the_daemon_no_longer_holds_is_told_to_resync() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let root = register(&daemon, &project).await;
    let mut stream = daemon.events(None).await;
    take(&daemon, &root).await;
    let (seen, _) = next_named(&mut stream, "snapshot.created").await;
    drop(stream);

    // A restart forgets every event: what happened while the daemon was down cannot be replayed.
    let daemon = daemon.restart().await;
    let mut resumed = daemon.events(seen.id.as_deref()).await;

    let first = next(&mut resumed).await;
    assert_eq!(first.event, "resync");
    assert_eq!(
        serde_json::from_str::<Value>(&first.data).expect("JSON"),
        serde_json::json!({ "event": "resync" })
    );
}

#[tokio::test]
async fn an_open_stream_ends_when_the_daemon_stops_rather_than_holding_it_up() {
    let daemon = Harness::start().await;
    let mut stream = daemon.events(None).await;

    tokio::time::timeout(PATIENCE, daemon.stop())
        .await
        .expect("the daemon stops with a stream open");

    // The stop is announced (ticket 67), and nothing follows it.
    let mut ended = tokio::time::timeout(PATIENCE, stream.next())
        .await
        .expect("the stream ends");
    if let Ok(Some(event)) = &ended {
        assert_eq!(event.event, "daemon.stopping");
        ended = tokio::time::timeout(PATIENCE, stream.next())
            .await
            .expect("the stream ends");
    }
    assert!(
        matches!(ended, Ok(None) | Err(_)),
        "no event after the stop: {ended:?}"
    );
}

#[tokio::test]
async fn registering_moving_and_unwatching_a_root_are_announced() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let mut stream = daemon.events(None).await;

    let root = register(&daemon, &daemon.scratch("project")).await;
    let (_, registered) = next_named(&mut stream, "root.registered").await;
    assert_eq!(
        registered,
        serde_json::json!({ "event": "root.registered", "root": root })
    );

    let moved_to = daemon.scratch("elsewhere");
    let moved = daemon
        .patch(
            &format!("/v1/roots/{root}"),
            &serde_json::json!({ "path": moved_to }),
        )
        .await;
    assert!(moved.status.is_success(), "{moved:?}");
    let (_, moved) = next_named(&mut stream, "root.moved").await;
    assert_eq!(
        moved,
        serde_json::json!({ "event": "root.moved", "root": root })
    );

    let gone = daemon.delete(&format!("/v1/roots/{root}?purge=true")).await;
    assert!(gone.status.is_success(), "{gone:?}");
    let (_, unregistered) = next_named(&mut stream, "root.unregistered").await;
    assert_eq!(
        unregistered,
        serde_json::json!({ "event": "root.unregistered", "root": root, "purged": true })
    );
}

#[tokio::test]
async fn a_root_whose_state_changes_is_announced_with_its_new_state() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    let root = register(&daemon, &project).await;
    let mut stream = daemon.events(None).await;

    std::fs::remove_dir(&project).expect("the directory removed");

    let (_, missing) = next_named(&mut stream, "root.state_changed").await;
    assert_eq!(
        missing,
        serde_json::json!({
            "event": "root.state_changed",
            "root": root,
            "state": "unavailable",
            "state_code": "root_missing",
        })
    );

    std::fs::create_dir(&project).expect("the directory back");

    let (_, back) = next_named(&mut stream, "root.state_changed").await;
    assert_eq!(back["state"], "degraded");
    assert_eq!(back["state_code"], "capture_unavailable");
}

#[tokio::test]
async fn a_configuration_edit_is_announced_as_reloaded_and_a_broken_one_as_invalid() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let root = register(&daemon, &daemon.scratch("project")).await;
    let mut stream = daemon.events(None).await;

    daemon.write_global_config(
        b"[triggers]\nactive = []\nrate_limit = \"5m\"\n\n[reconcile]\nat_start = false\n",
    );
    let (_, reloaded) = next_named(&mut stream, "config.reloaded").await;
    assert_eq!(
        reloaded,
        serde_json::json!({ "event": "config.reloaded", "root": root })
    );

    daemon.write_global_config(b"[triggers\n");
    let (_, invalid) = next_named(&mut stream, "config.invalid").await;
    assert_eq!(
        invalid,
        serde_json::json!({ "event": "config.invalid", "root": root })
    );
    let (_, degraded) = next_named(&mut stream, "root.state_changed").await;
    assert_eq!(degraded["state_code"], "config_invalid");
}
