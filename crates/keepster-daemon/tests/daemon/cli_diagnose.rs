//! `keepster diagnose` as the shipped binary runs it (ticket 63, ADR 0013): one local zip with fixed
//! content, an inventory printed before it is written, a bundle collected from the disk when no daemon
//! answers — which names what it lacks — and never a byte of a file from history.
#![cfg(unix)]

use std::path::Path;
use std::process::Output;

use serde_json::json;

use crate::cli_lifecycle::{Sandbox, signal};

/// What a captured file holds, chosen so that it appears nowhere else.
const CANARY: &str = "keepster-canary-5d1c7a20-content-of-a-captured-file";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Whether `archive` holds `needle` anywhere: its entries are stored, not compressed.
fn holds(archive: &[u8], needle: &str) -> bool {
    archive
        .windows(needle.len())
        .any(|window| window == needle.as_bytes())
}

async fn diagnose(sandbox: &Sandbox, directory: &Path, args: &[&str]) -> Output {
    let mut command = sandbox.command(&[&["diagnose"], args].concat());
    command.current_dir(directory);
    command.output().await.expect("keepster diagnose runs")
}

/// A sandbox with a running daemon and one Root holding [`CANARY`] in one Snapshot.
async fn with_history(sandbox: &Sandbox) -> (tokio::process::Child, String) {
    let daemon = sandbox.daemon().await;
    let project = sandbox.scratch("project");
    std::fs::write(project.join("notes.txt"), CANARY).expect("a file");
    let client = sandbox.client();
    let registered = client
        .post(
            "/v1/roots",
            json!({ "path": project }).to_string().as_bytes(),
        )
        .await
        .expect("registered");
    let root: serde_json::Value = serde_json::from_slice(&registered.body).expect("JSON");
    let root = root["id"].as_str().expect("an id").to_owned();
    client
        .post(&format!("/v1/roots/{root}/snapshots"), b"{}")
        .await
        .expect("captured");

    (daemon, root)
}

#[tokio::test]
async fn a_bundle_is_one_local_zip_listed_before_it_is_written() {
    let sandbox = Sandbox::new();
    let (_daemon, root) = with_history(&sandbox).await;
    let out = sandbox.scratch("out");

    let ran = diagnose(&sandbox, &out, &["-o", "bundle.zip"]).await;

    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    let inventory = text(&ran.stdout);
    for name in [
        "inventory.txt",
        "versions.json",
        "status.json",
        "environment.json",
        "configuration.json",
        "registry.json",
        "topology.json",
        "watermarks.json",
        "logs/daemon.log",
    ] {
        assert!(inventory.contains(name), "`{name}`:\n{inventory}");
    }
    assert!(inventory.contains("Log format: text"), "{inventory}");
    assert!(
        inventory.contains("Nothing in it has been sent anywhere"),
        "{inventory}"
    );
    let archive = std::fs::read(out.join("bundle.zip")).expect("the bundle is written");
    assert!(holds(&archive, "registry.json") && holds(&archive, &root));
    assert!(
        holds(&archive, "the daemon is starting"),
        "the log's tail is in it"
    );
    assert!(
        !holds(&archive, CANARY),
        "the content of a captured file is in the bundle"
    );
}

#[tokio::test]
async fn without_a_daemon_the_disk_is_collected_and_the_inventory_names_what_is_missing() {
    let sandbox = Sandbox::new();
    let (mut daemon, root) = with_history(&sandbox).await;
    signal(daemon.id().expect("running"), "TERM");
    daemon.wait().await.expect("it stops");
    let out = sandbox.scratch("out");

    let ran = diagnose(&sandbox, &out, &["-o", "bundle.zip"]).await;

    assert_eq!(
        ran.status.code(),
        Some(3),
        "a degraded bundle exits as an unreachable daemon does: {}",
        text(&ran.stderr)
    );
    let inventory = text(&ran.stdout);
    assert!(
        inventory.contains("Missing from this bundle"),
        "{inventory}"
    );
    for missing in ["status.json:", "topology.json:", "watermarks.json:"] {
        assert!(inventory.contains(missing), "`{missing}`:\n{inventory}");
    }
    let archive = std::fs::read(out.join("bundle.zip")).expect("the bundle is written");
    assert!(holds(&archive, &root), "the Registry is read from the disk");
    assert!(holds(&archive, "configuration.json"));
    assert!(holds(&archive, "logs/daemon.log"), "and the log");
    assert!(!holds(&archive, CANARY));
    assert!(
        sandbox.client().get("/v1/version").await.is_err(),
        "collecting started no daemon"
    );
}

#[tokio::test]
async fn a_dry_run_prints_the_inventory_and_writes_nothing() {
    let sandbox = Sandbox::new();
    let _daemon = sandbox.daemon().await;
    let out = sandbox.scratch("out");

    let ran = diagnose(&sandbox, &out, &["--dry-run"]).await;

    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    assert!(text(&ran.stdout).contains("status.json"));
    assert_eq!(
        std::fs::read_dir(&out).expect("listed").count(),
        0,
        "nothing is written"
    );
}

#[tokio::test]
async fn an_existing_file_is_never_overwritten() {
    let sandbox = Sandbox::new();
    let _daemon = sandbox.daemon().await;
    let out = sandbox.scratch("out");
    std::fs::write(out.join("bundle.zip"), b"someone's file").expect("a file");

    let ran = diagnose(&sandbox, &out, &["-o", "bundle.zip"]).await;

    assert_eq!(ran.status.code(), Some(5), "{}", text(&ran.stderr));
    assert!(
        text(&ran.stderr).contains("already exists"),
        "{}",
        text(&ran.stderr)
    );
    assert_eq!(
        std::fs::read(out.join("bundle.zip")).expect("still there"),
        b"someone's file"
    );
}

#[tokio::test]
async fn with_no_name_given_the_bundle_is_named_for_the_moment_it_was_written() {
    let sandbox = Sandbox::new();
    let _daemon = sandbox.daemon().await;
    let out = sandbox.scratch("out");

    let ran = diagnose(&sandbox, &out, &[]).await;

    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    let written: Vec<String> = std::fs::read_dir(&out)
        .expect("listed")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(written.len(), 1, "{written:?}");
    assert!(
        written[0].starts_with("keepster-diagnose-") && written[0].ends_with(".zip"),
        "{written:?}"
    );
}

#[tokio::test]
async fn without_a_daemon_a_configuration_that_cannot_be_read_is_named_missing_for_the_roots() {
    let sandbox = Sandbox::new();
    let (mut daemon, _root) = with_history(&sandbox).await;
    signal(daemon.id().expect("running"), "TERM");
    daemon.wait().await.expect("it stops");
    sandbox.write_config(b"[log\n");
    let out = sandbox.scratch("out");

    let ran = diagnose(&sandbox, &out, &["--dry-run"]).await;

    assert_eq!(ran.status.code(), Some(3), "{}", text(&ran.stderr));
    let inventory = text(&ran.stdout);
    let missing = inventory
        .split("Missing from this bundle:")
        .nth(1)
        .expect("a list of what is missing");
    assert!(
        missing.contains("configuration.json (each Root's configuration):"),
        "{inventory}"
    );
}

#[tokio::test]
async fn without_a_daemon_the_log_format_is_the_one_its_lines_were_written_in() {
    let sandbox = Sandbox::new();
    let mut daemon = sandbox.daemon().await;
    signal(daemon.id().expect("running"), "TERM");
    daemon.wait().await.expect("it stops");
    // Edited after the daemon wrote its log: the next start would take it, the lines already written
    // keep theirs.
    sandbox.write_config(b"[log]\nformat = \"json\"\n");
    let out = sandbox.scratch("out");

    let ran = diagnose(&sandbox, &out, &["--dry-run"]).await;

    let inventory = text(&ran.stdout);
    assert!(
        inventory.contains("Log format: text, read from the log's own lines"),
        "{inventory}"
    );
}

/// A line of the tray's log, chosen so that it appears nowhere else.
const TRAY_LINE: &str = "INFO keepster_tray::run: tray-canary-0b6f connected to the daemon";

/// The tray's log beside the daemon's, as a tray running in the sandbox would write it.
fn write_tray_log(sandbox: &Sandbox) {
    let file = sandbox.log_file().with_file_name("tray.log");
    std::fs::create_dir_all(file.parent().expect("a directory")).expect("the log directory");
    std::fs::write(file, format!("{TRAY_LINE}\n")).expect("the tray's log");
}

#[tokio::test]
async fn a_bundle_holds_the_tray_s_log_beside_the_daemon_s() {
    let sandbox = Sandbox::new();
    let (_daemon, _root) = with_history(&sandbox).await;
    write_tray_log(&sandbox);
    let out = sandbox.scratch("out");

    let ran = diagnose(&sandbox, &out, &["-o", "bundle.zip"]).await;

    assert_eq!(ran.status.code(), Some(0), "{}", text(&ran.stderr));
    let inventory = text(&ran.stdout);
    assert!(inventory.contains("logs/tray.log"), "{inventory}");
    assert!(inventory.contains("the tray's log"), "{inventory}");
    let archive = std::fs::read(out.join("bundle.zip")).expect("the bundle is written");
    assert!(holds(&archive, TRAY_LINE), "the tray's lines are in it");
}

#[tokio::test]
async fn without_a_daemon_the_tray_s_log_is_collected_from_the_disk_too() {
    let sandbox = Sandbox::new();
    write_tray_log(&sandbox);
    let out = sandbox.scratch("out");

    let ran = diagnose(&sandbox, &out, &["-o", "bundle.zip"]).await;

    assert_eq!(ran.status.code(), Some(3), "{}", text(&ran.stderr));
    assert!(
        text(&ran.stdout).contains("logs/tray.log"),
        "{}",
        text(&ran.stdout)
    );
    let archive = std::fs::read(out.join("bundle.zip")).expect("the bundle is written");
    assert!(holds(&archive, TRAY_LINE), "the tray's lines are in it");
}
