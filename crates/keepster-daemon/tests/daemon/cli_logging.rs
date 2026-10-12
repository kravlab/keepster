//! The daemon's log as the shipped binary writes it (ticket 62): one file in the user's log directory,
//! the journal told only that the daemon started, and a level raised from the CLI for a while.
#![cfg(unix)]

use tokio::io::AsyncReadExt;

use crate::cli_lifecycle::{Sandbox, eventually, json, signal};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn log_of(sandbox: &Sandbox) -> String {
    std::fs::read_to_string(sandbox.log_file()).unwrap_or_default()
}

#[tokio::test]
async fn the_daemon_writes_its_log_where_the_platform_keeps_a_users_logs() {
    let sandbox = Sandbox::new();
    let _daemon = sandbox.daemon().await;

    let status = json(&sandbox.client(), "/v1/status").await;

    assert_eq!(
        status["daemon"]["log"]["file"],
        sandbox.log_file().display().to_string().as_str(),
        "{status}"
    );
    assert!(
        log_of(&sandbox).contains("the daemon is starting"),
        "{}",
        log_of(&sandbox)
    );
}

#[tokio::test]
async fn standard_error_says_the_daemon_started_and_nothing_else() {
    // Under systemd standard error is the journal: it hears that the daemon started and why it died,
    // never the volume the file is for.
    let sandbox = Sandbox::new();
    let mut daemon = sandbox.daemon().await;
    let client = sandbox.client();
    client
        .put_json("/v1/log/level", br#"{"level":"trace","for":"1h"}"#)
        .await
        .expect("raised");
    for _ in 0..5 {
        json(&client, "/v1/status").await;
    }

    signal(daemon.id().expect("running"), "TERM");
    daemon.wait().await.expect("it exits");
    let mut said = String::new();
    daemon
        .stderr
        .take()
        .expect("standard error")
        .read_to_string(&mut said)
        .await
        .expect("read");

    assert_eq!(said.lines().count(), 1, "{said}");
    assert!(said.contains("is running as pid"), "{said}");
    assert!(
        said.contains(&sandbox.log_file().display().to_string()),
        "{said}"
    );
    assert!(
        log_of(&sandbox).contains("answered"),
        "the volume went to the file"
    );
}

#[tokio::test]
async fn a_level_raised_from_the_cli_writes_what_the_default_leaves_out_until_it_ends() {
    let sandbox = Sandbox::new();
    let _daemon = sandbox.daemon().await;
    let client = sandbox.client();
    json(&client, "/v1/version").await;
    assert!(
        !log_of(&sandbox).contains("answered"),
        "nothing at debug by default"
    );

    let raised = sandbox.run(&["log-level", "debug", "--for", "1h"]).await;

    assert_eq!(raised.status.code(), Some(0), "{}", text(&raised.stderr));
    let shown = text(&raised.stdout);
    assert!(shown.contains("debug until"), "{shown}");
    assert!(shown.contains("then info"), "{shown}");
    json(&client, "/v1/version").await;
    eventually("a debug line in the file", || async {
        log_of(&sandbox).contains("path=/v1/version").then_some(())
    })
    .await;
    let status = sandbox.run(&["status"]).await;
    assert!(
        text(&status.stdout).contains("debug until"),
        "{}",
        text(&status.stdout)
    );

    let reset = sandbox.run(&["log-level", "--reset"]).await;

    assert_eq!(reset.status.code(), Some(0), "{}", text(&reset.stderr));
    let row = json(&client, "/v1/status").await["daemon"]["log"].clone();
    assert_eq!(row["level"], "info", "{row}");
}

#[tokio::test]
async fn a_raise_without_a_duration_is_refused_by_the_cli_itself() {
    let sandbox = Sandbox::new();

    let refused = sandbox.run(&["log-level", "debug"]).await;

    assert_eq!(
        refused.status.code(),
        Some(2),
        "a usage error: {}",
        text(&refused.stderr)
    );
    assert!(
        text(&refused.stderr).contains("--for"),
        "{}",
        text(&refused.stderr)
    );
}

#[tokio::test]
async fn the_json_format_writes_one_object_per_line_in_the_same_file() {
    let sandbox = Sandbox::new();
    sandbox.write_config(b"[log]\nformat = \"json\"\n");
    let _daemon = sandbox.daemon().await;

    let written = log_of(&sandbox);

    assert!(!written.is_empty());
    for line in written.lines() {
        let record: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}: {line}"));
        assert!(record["level"].is_string(), "{line}");
    }
}

#[tokio::test]
async fn the_ceiling_bounds_every_file_and_how_many_are_kept() {
    let sandbox = Sandbox::new();
    sandbox.write_config(b"[log]\nceiling = \"64 KiB\"\n");
    let _daemon = sandbox.daemon().await;
    let client = sandbox.client();
    client
        .put_json("/v1/log/level", br#"{"level":"debug","for":"1h"}"#)
        .await
        .expect("raised");

    for _ in 0..1500 {
        client.get("/v1/version").await.expect("answered");
    }

    let directory = sandbox
        .log_file()
        .parent()
        .expect("a directory")
        .to_path_buf();
    eventually("the log to have rotated", || async {
        directory.join("daemon.log.4").exists().then_some(())
    })
    .await;
    // Back to `info`, where a request writes nothing: the files hold still while they are measured.
    client.delete("/v1/log/level").await.expect("ended");
    let mut total = 0;
    for name in [
        "daemon.log",
        "daemon.log.1",
        "daemon.log.2",
        "daemon.log.3",
        "daemon.log.4",
    ] {
        let size = std::fs::metadata(directory.join(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"))
            .len();
        // A file is rotated before the line that would take it past its share of the ceiling.
        assert!(size <= 64 * 1024 / 5, "{name} holds {size} bytes");
        total += size;
    }
    assert!(
        !directory.join("daemon.log.5").exists(),
        "four rotated files, no more"
    );
    assert!(total <= 64 * 1024, "{total} bytes in all");
}
