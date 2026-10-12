//! Volume identity as the gate of capture, through `/v1` (ticket 56, ADR 0011): a Root whose volume is
//! gone is `Unavailable` while its history reads as ever, a volume mounted somewhere else is followed,
//! a total deletion nothing can vouch for is held, a filesystem the watcher is blind on is said to be,
//! and a store that cannot be written is never replaced by another.

use std::path::Path;
use std::time::Duration;

use keepster_daemon::HousekeepingTiming;
use keepster_testkit::{Harness, MANUAL};
use serde_json::{Value, json};

fn json(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

async fn register(daemon: &Harness, path: &Path) -> String {
    daemon
        .post_json("/v1/roots", &json!({ "path": path }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned()
}

async fn take(daemon: &Harness, root: &str) -> keepster_cli::Response {
    daemon
        .post(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await
}

async fn root_of(daemon: &Harness, root: &str) -> Value {
    daemon.get_json(&format!("/v1/roots/{root}")).await
}

/// A directory holding `notes.txt` and `src/main.rs`.
fn project_in(parent: &Path) -> std::path::PathBuf {
    let project = parent.join("project");
    std::fs::create_dir_all(project.join("src")).expect("the project");
    std::fs::write(project.join("notes.txt"), b"kept").expect("a file");
    std::fs::write(project.join("src").join("main.rs"), b"fn main() {}").expect("a file");
    project
}

#[tokio::test]
async fn a_root_whose_volume_is_gone_is_unavailable_and_its_history_still_reads() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let disk = daemon.scratch("disk");
    daemon.volumes().mount(&disk, Some("uuid:disk"));
    let root = register(&daemon, &project_in(&disk)).await;
    let taken = take(&daemon, &root).await;
    assert_eq!(taken.status.as_u16(), 201);
    let snapshot = json(&taken)["id"].as_str().expect("an id").to_owned();

    // Unplugged: the mount point is an ordinary directory of the volume beneath it.
    daemon.volumes().unmount(&disk);

    let state = root_of(&daemon, &root).await;
    assert_eq!(state["state"], "unavailable", "{state}");
    assert_eq!(state["state_code"], "volume_mismatch", "{state}");
    let refused = take(&daemon, &root).await;
    assert_eq!(refused.status.as_u16(), 409);
    assert_eq!(json(&refused)["code"], "volume_mismatch");
    let restore = daemon
        .post(
            &format!("/v1/roots/{root}/restore"),
            &json!({ "from": snapshot }),
        )
        .await;
    assert_eq!(restore.status.as_u16(), 409, "nothing is written into it");
    assert_eq!(json(&restore)["code"], "volume_mismatch");

    // `Unavailable` is not unreadable.
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(
        timeline["snapshots"][0]["id"],
        snapshot.as_str(),
        "{timeline}"
    );
    let read = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/content?path=notes.txt"
        ))
        .await;
    assert_eq!(read.status.as_u16(), 200);
    assert_eq!(read.body, b"kept");

    // Plugged in again where it was: watched as before, with nobody doing anything.
    daemon.volumes().mount(&disk, Some("uuid:disk"));
    assert_eq!(take(&daemon, &root).await.status.as_u16(), 201);
}

#[tokio::test]
async fn a_volume_mounted_somewhere_else_is_followed_with_the_root_at_the_same_place_within_it() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let disk = daemon.scratch("media").join("disk");
    let project = project_in(&disk);
    daemon.volumes().mount(&disk, Some("uuid:disk"));
    let root = register(&daemon, &project).await;
    assert_eq!(take(&daemon, &root).await.status.as_u16(), 201);

    // Unplugged, and plugged in again where the system chose. Its old mount point stays behind as an
    // ordinary directory, as it does on a real machine.
    daemon.volumes().unmount(&disk);
    let elsewhere = daemon.scratch("run").join("DISK1");
    project_in(&elsewhere);
    daemon.volumes().mount(&elsewhere, Some("uuid:disk"));

    let taken = take(&daemon, &root).await;

    assert_eq!(taken.status.as_u16(), 201, "captured where it is now");
    let state = root_of(&daemon, &root).await;
    assert_eq!(
        state["path"],
        elsewhere.join("project").to_string_lossy().as_ref(),
        "{state}"
    );
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(
        timeline["snapshots"].as_array().map(Vec::len),
        Some(2),
        "one history, followed rather than started again: {timeline}"
    );
}

#[tokio::test]
async fn a_volume_mounted_inside_another_root_is_not_followed_there() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let disk = daemon.scratch("media").join("disk");
    daemon.volumes().mount(&disk, Some("uuid:disk"));
    let root = register(&daemon, &project_in(&disk)).await;
    let run = daemon.scratch("run");
    register(&daemon, &run).await;

    daemon.volumes().unmount(&disk);
    let inside = run.join("DISK1");
    project_in(&inside);
    daemon.volumes().mount(&inside, Some("uuid:disk"));

    let refused = take(&daemon, &root).await;

    assert_eq!(
        refused.status.as_u16(),
        409,
        "the no-overlap rule holds for a followed volume too"
    );
    let state = root_of(&daemon, &root).await;
    assert_eq!(
        state["path"],
        disk.join("project").to_string_lossy().as_ref(),
        "{state}"
    );
    assert_eq!(state["state"], "unavailable", "{state}");
}

#[tokio::test]
async fn a_volume_mounted_somewhere_else_is_followed_without_waiting_for_a_capture() {
    let quick = HousekeepingTiming {
        every: Duration::from_secs(1),
        quiet: Duration::from_secs(3600),
        ceiling: Duration::from_secs(3600),
        maintain_every: Duration::from_secs(24 * 3600),
    };
    let daemon = Harness::start_with_housekeeping(quick, MANUAL).await;
    let disk = daemon.scratch("media").join("disk");
    let project = project_in(&disk);
    daemon.volumes().mount(&disk, Some("uuid:disk"));
    let root = register(&daemon, &project).await;

    daemon.volumes().unmount(&disk);
    let elsewhere = daemon.scratch("run").join("DISK1");
    project_in(&elsewhere);
    daemon.volumes().mount(&elsewhere, Some("uuid:disk"));

    let wanted = elsewhere.join("project").to_string_lossy().into_owned();
    let state = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let state = root_of(&daemon, &root).await;
            if state["path"] == wanted.as_str() {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("followed at the next look");
    assert_ne!(state["state_code"], "root_missing", "{state}");
}

#[tokio::test]
async fn where_the_volume_has_no_identity_an_automatic_total_deletion_is_held_until_one_is_asked_for()
 {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let anonymous = daemon.scratch("anonymous");
    daemon.volumes().mount(&anonymous, None);
    let project = project_in(&anonymous);
    let root = register(&daemon, &project).await;
    assert_eq!(take(&daemon, &root).await.status.as_u16(), 201);
    std::fs::remove_file(project.join("notes.txt")).expect("gone");
    std::fs::remove_dir_all(project.join("src")).expect("gone");

    // A Reconciliation is no one's confirmation.
    let reconciled = daemon
        .post_json(&format!("/v1/roots/{root}/reconcile"), &json!({}))
        .await;

    assert!(reconciled.get("snapshot").is_none(), "{reconciled}");
    let state = root_of(&daemon, &root).await;
    assert_eq!(state["state"], "degraded", "{state}");
    assert_eq!(state["state_code"], "capture_held", "{state}");
    let snapshots = |timeline: Value| timeline["snapshots"].as_array().map(Vec::len);
    let url = format!("/v1/roots/{root}/snapshots");
    assert_eq!(snapshots(daemon.get_json(&url).await), Some(1));

    // A capture someone asks for is the confirmation.
    assert_eq!(take(&daemon, &root).await.status.as_u16(), 201);
    assert_eq!(snapshots(daemon.get_json(&url).await), Some(2));
    let state = root_of(&daemon, &root).await;
    assert_ne!(state["state_code"], "capture_held", "{state}");
}

#[tokio::test]
async fn a_root_on_a_filesystem_the_watcher_is_blind_on_is_degraded_exactly_where_it_is() {
    let daemon = Harness::start().await;
    let share = daemon.scratch("share");
    daemon
        .volumes()
        .mount_network(&share, "fuse.sshfs", "remote:host:/srv/share");
    let root = register(&daemon, &project_in(&share)).await;

    // Linux's inotify and macOS's FSEvents see no remote change; Windows documents SMB as watched.
    let blind = !cfg!(windows);
    let state = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let state = root_of(&daemon, &root).await;
            let settled = if blind {
                state["state_code"] == "watch_blind"
            } else {
                state["state"] == "watching"
            };
            if settled {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the Root settles");
    assert_eq!(state["state"], if blind { "degraded" } else { "watching" });
}

#[tokio::test]
async fn a_store_that_cannot_be_written_stops_capture_and_nothing_is_written_elsewhere() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = project_in(&daemon.scratch("plain"));
    let root = register(&daemon, &project).await;
    let store = daemon.store_dir(&root).join("history.git");
    std::fs::write(&store, b"not a store").expect("a file where the store goes");

    let refused = take(&daemon, &root).await;

    assert_eq!(refused.status.as_u16(), 409);
    assert_eq!(json(&refused)["code"], "store_unreachable");
    let state = root_of(&daemon, &root).await;
    assert_eq!(state["state"], "unavailable", "{state}");
    assert_eq!(state["state_code"], "store_unreachable", "{state}");
    assert!(store.is_file(), "left as it was");
    let roots: Vec<_> = std::fs::read_dir(daemon.store_dir(&root).parent().expect("the base"))
        .expect("the store base")
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(
        roots,
        vec![std::ffi::OsString::from(&root)],
        "no other store"
    );
}

#[tokio::test]
async fn watching_a_root_again_records_the_volume_it_is_on_now() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let disk = daemon.scratch("disk");
    daemon.volumes().mount(&disk, Some("uuid:old"));
    let project = project_in(&disk);
    let root = register(&daemon, &project).await;
    let unwatched = daemon.delete(&format!("/v1/roots/{root}")).await;
    assert!(
        unwatched.status.is_success(),
        "{}",
        String::from_utf8_lossy(&unwatched.body)
    );

    // Another disk at the same place, and the person watches it again: registered again, as it is now.
    daemon.volumes().mount(&disk, Some("uuid:new"));
    let again = register(&daemon, &project).await;

    assert_eq!(again, root, "the same history, adopted again");
    assert_eq!(take(&daemon, &again).await.status.as_u16(), 201);
    assert_eq!(
        root_of(&daemon, &again).await["state_code"],
        "capture_unavailable"
    );
}
