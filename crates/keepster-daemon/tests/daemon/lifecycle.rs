//! One supervised daemon per user (ticket 61): the pid file's lock is the single-instance authority,
//! taken before anything touches a store, and a socket a crash left behind is no obstacle.

use keepster_testkit::Harness;

#[tokio::test]
async fn a_second_daemon_is_refused_and_names_the_live_one() {
    let daemon = Harness::start().await;

    let refused = daemon.start_another().await;

    match refused {
        Err(keepster_daemon::StartError::AlreadyRunning { pid }) => {
            assert_eq!(pid, Some(std::process::id()), "the live daemon's pid");
        }
        Err(other) => panic!("refused for another reason: {other}"),
        Ok(_) => panic!("two daemons over one runtime directory"),
    }
    let status = daemon.get("/v1/status").await;
    assert_eq!(status.status, 200, "the live daemon serves on");
}

#[cfg(unix)]
#[tokio::test]
async fn a_refused_second_daemon_touches_no_index() {
    // Recovery discards an index it finds damaged or missing and builds it again, which beside a running
    // daemon would unlink a database that daemon holds open (ticket 39). An index taken away from under
    // the live daemon is how a test sees whether recovery ran: it would come back.
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.md"), "kept").expect("a file");
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;
    let id = root["id"].as_str().expect("an id").to_owned();
    daemon
        .post_json(&format!("/v1/roots/{id}/snapshots"), &serde_json::json!({}))
        .await;
    let index = daemon.store_dir(&id).join("index.sqlite");
    std::fs::remove_file(&index).expect("the index, taken away");

    let refused = daemon.start_another().await;

    assert!(refused.is_err(), "refused");
    assert!(!index.exists(), "the refused daemon recovered no index");
}

#[cfg(unix)]
#[tokio::test]
async fn a_socket_a_crash_left_behind_is_replaced() {
    let daemon = Harness::start().await;
    let stopped = daemon.stop().await;
    let socket = stopped.control_path();
    // What a killed daemon leaves: the socket file, with nothing listening on it and no lock held.
    if !socket.exists() {
        drop(std::os::unix::net::UnixListener::bind(&socket).expect("a socket file"));
    }
    assert!(
        std::os::unix::net::UnixStream::connect(&socket).is_err(),
        "nothing listens on it"
    );

    let daemon = stopped.start().await;

    assert_eq!(daemon.get("/v1/status").await.status, 200);
}
