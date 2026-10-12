//! Configuration at the `/v1` seam (ticket 42): the effective configuration with per-key provenance,
//! automatic reload, the last known-good, and what an invalid file does to a Root.

use std::path::{Path, PathBuf};

use keepster_testkit::Harness;
use serde_json::Value;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

/// Write `content` at `path` as an editor saves: beside it, then renamed over it. A plain write truncates
/// first, and the daemon — which re-reads its files every second — could read the empty file in between
/// and take it, validly, for the configuration of the moment.
fn write(root: &Path, path: &str, content: &[u8]) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    let temporary = file.with_extension("saving");
    std::fs::write(&temporary, content).expect("a file");
    std::fs::rename(temporary, file).expect("saved");
}

async fn registered(daemon: &Harness) -> (String, PathBuf) {
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    (root["id"].as_str().expect("an id").to_owned(), project)
}

async fn configuration(daemon: &Harness, root: &str) -> Value {
    daemon.get_json(&format!("/v1/roots/{root}/config")).await
}

fn key<'a>(configuration: &'a Value, name: &str) -> &'a Value {
    configuration["keys"]
        .as_array()
        .expect("the keys")
        .iter()
        .find(|key| key["key"] == name)
        .unwrap_or_else(|| panic!("`{name}` is reported: {configuration}"))
}

async fn state(daemon: &Harness, root: &str) -> (String, String) {
    let shown = daemon.get_json(&format!("/v1/roots/{root}")).await;

    (
        shown["state"].as_str().expect("a state").to_owned(),
        shown["state_code"].as_str().unwrap_or_default().to_owned(),
    )
}

/// Wait until the Root is `Watching`, which it is once its first Reconciliation has run — the state this
/// file's tests compare a configuration's effect against.
async fn watching(daemon: &Harness, root: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while state(daemon, root).await.0 != "watching" {
        assert!(
            std::time::Instant::now() < deadline,
            "the Root never became watching: {:?}",
            state(daemon, root).await
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

async fn snapshot(daemon: &Harness, root: &str) -> keepster_cli::Response {
    daemon
        .post(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await
}

#[tokio::test]
async fn a_root_nobody_configured_reports_every_key_at_its_documented_default() {
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;

    let configuration = configuration(&daemon, &root).await;

    assert_eq!(configuration["root"], root.as_str());
    let max_size = key(&configuration, "files.max_size");
    assert_eq!(max_size["value"], 100 * 1024 * 1024);
    assert_eq!(max_size["origin"]["level"], "default");
    assert_eq!(max_size["restart_required"], false);
    assert_eq!(
        key(&configuration, "files.respect_gitignore")["value"],
        false
    );
    assert_eq!(
        key(&configuration, "files.exclude")["value"],
        serde_json::json!([])
    );
    assert_eq!(configuration["errors"], serde_json::json!([]));
}

#[tokio::test]
async fn every_level_is_reported_as_the_one_that_set_the_key() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    let glob = format!("{}/*", project.parent().expect("a parent").display()).replace('\\', "/");
    daemon.write_global_config(
        format!(
            "[files]\nexclude = [\"*.log\"]\nmax_size = \"1 MiB\"\n\n\
             [[roots]]\npath = \"{glob}\"\n[roots.files]\nrespect_gitignore = true\n\n\
             [root.{root}.files]\nsecret_opt_in = [\".env.example\"]\n"
        )
        .as_bytes(),
    );
    write(
        &project,
        ".keepster.toml",
        b"[files]\nexclude = [\"!keep.log\"]\n",
    );

    let configuration = configuration(&daemon, &root).await;

    assert_eq!(
        key(&configuration, "files.max_size")["origin"]["level"],
        "global"
    );
    assert_eq!(key(&configuration, "files.max_size")["origin"]["line"], 3);
    assert_eq!(
        key(&configuration, "files.respect_gitignore")["origin"]["level"],
        "global-pattern"
    );
    assert_eq!(
        key(&configuration, "files.secret_opt_in")["origin"]["level"],
        "global-root"
    );
    let exclude = key(&configuration, "files.exclude");
    assert_eq!(exclude["value"], serde_json::json!(["*.log", "!keep.log"]));
    assert_eq!(exclude["origin"]["level"], "root");
    assert_eq!(exclude["entries"][0]["level"], "global");
    assert_eq!(exclude["entries"][1]["level"], "root");
    assert_eq!(exclude["entries"][1]["line"], 2);
}

#[tokio::test]
async fn an_edit_to_either_file_takes_effect_without_a_restart() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(&project, "debug.log", b"noise");
    write(&project, "notes.txt", b"kept");

    daemon.write_global_config(b"[files]\nexclude = [\"*.log\"]\n");
    let explained = daemon
        .get_json(&format!("/v1/roots/{root}/explain?path=debug.log"))
        .await;
    assert_eq!(explained["decision"], "excluded");

    write(
        &project,
        ".keepster.toml",
        b"[files]\nexclude = [\"*.txt\"]\n",
    );
    let explained = daemon
        .get_json(&format!("/v1/roots/{root}/explain?path=notes.txt"))
        .await;
    assert_eq!(explained["decision"], "excluded");
}

#[tokio::test]
async fn an_invalid_edit_leaves_the_last_known_good_in_force_and_the_root_degraded() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(&project, "debug.log", b"noise");
    write(&project, "main.rs", b"fn main() {}");
    write(
        &project,
        ".keepster.toml",
        b"[files]\nexclude = [\"*.log\"]\n",
    );
    watching(&daemon, &root).await;

    write(
        &project,
        ".keepster.toml",
        b"[files]\nexlude = [\"*.log\"]\n",
    );

    assert_eq!(
        state(&daemon, &root).await,
        ("degraded".to_owned(), "config_invalid".to_owned())
    );
    let configuration = configuration(&daemon, &root).await;
    assert_eq!(
        key(&configuration, "files.exclude")["value"],
        serde_json::json!(["*.log"]),
        "the last known-good is what is in force"
    );
    let errors = configuration["errors"].as_array().expect("the errors");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["code"], "config_invalid");
    assert!(
        errors[0]["details"]["path"]
            .as_str()
            .expect("the file")
            .ends_with(".keepster.toml")
    );

    let taken = snapshot(&daemon, &root).await;
    assert_eq!(taken.status, 201, "a Degraded Root is still captured");
    let taken: Value = serde_json::from_slice(&taken.body).expect("a Snapshot");
    let entries = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots/{}/entries?depth=infinity",
            taken["id"].as_str().expect("an id")
        ))
        .await;
    let paths: Vec<&str> = entries["entries"]
        .as_array()
        .expect("the entries")
        .iter()
        .map(|entry| entry["path"].as_str().expect("a path"))
        .collect();
    assert!(paths.contains(&"main.rs"));
    assert!(
        !paths.contains(&"debug.log"),
        "the known-good rules still exclude it"
    );

    write(
        &project,
        ".keepster.toml",
        b"[files]\nexclude = [\"*.log\"]\n",
    );
    watching(&daemon, &root).await;
}

#[tokio::test]
async fn with_no_known_good_version_the_root_is_not_captured_at_all() {
    // Broken from the first time the daemon reads it, so no version was ever valid — not even the
    // defaults an absent file would have meant.
    let daemon = Harness::start_with_global_config(b"[files]\nexclude = \"*.log\"\n").await;
    let (root, _) = registered(&daemon).await;

    assert_eq!(
        state(&daemon, &root).await,
        ("unavailable".to_owned(), "config_invalid".to_owned())
    );
    let refused = snapshot(&daemon, &root).await;
    assert_eq!(refused.status, 409);
    let problem: Value = serde_json::from_slice(&refused.body).expect("a problem");
    assert_eq!(problem["code"], "config_invalid");

    let shown = daemon.get(&format!("/v1/roots/{root}/config")).await;
    assert_eq!(shown.status, 409);
}

#[tokio::test]
async fn the_last_known_good_survives_a_restart() {
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;
    daemon.write_global_config(b"[files]\nmax_size = \"1 MiB\"\n");
    configuration(&daemon, &root).await;
    daemon.write_global_config(b"[files]\nmax_size = \"lots\"\n");

    let daemon = daemon.restart().await;

    assert_eq!(
        state(&daemon, &root).await,
        ("degraded".to_owned(), "config_invalid".to_owned())
    );
    let configuration = configuration(&daemon, &root).await;
    assert_eq!(key(&configuration, "files.max_size")["value"], 1024 * 1024);
}

#[tokio::test]
async fn a_root_file_setting_a_global_only_key_is_ignored_and_the_root_not_degraded() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(
        &project,
        ".keepster.toml",
        b"[files]\nsecret_opt_in = [\".env\"]\n",
    );

    let configuration = configuration(&daemon, &root).await;

    assert_eq!(
        key(&configuration, "files.secret_opt_in")["value"],
        serde_json::json!([])
    );
    assert_eq!(configuration["warnings"][0]["code"], "key_ignored");
    watching(&daemon, &root).await;
}

#[tokio::test]
async fn keepster_config_prints_the_effective_values_and_where_each_was_set() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    daemon.write_global_config(b"[files]\nexclude = [\"*.log\"]\n");

    let printed = daemon.run_cli_in(&project, KEEPSTER, &["config"]).await;

    assert_eq!(printed.exit_code, Some(0), "{}", printed.stderr);
    let text = String::from_utf8(printed.stdout).expect("text");
    assert!(text.contains("files.exclude"), "{text}");
    assert!(text.contains("*.log"), "{text}");
    assert!(text.contains("global"), "{text}");
    assert!(
        text.contains("files.max_size = 104857600 (default)"),
        "{text}"
    );

    let json = daemon
        .run_cli(KEEPSTER, &["config", "--root", &root, "--json"])
        .await;
    assert_eq!(json.exit_code, Some(0), "{}", json.stderr);
    let parsed: Value = serde_json::from_slice(&json.stdout).expect("the daemon's JSON, verbatim");
    assert_eq!(parsed["root"], root.as_str());
}

#[tokio::test]
async fn a_file_created_broken_falls_back_to_the_defaults_its_absence_meant() {
    // No file was a valid configuration — the defaults — so a broken one replacing it leaves the Root
    // Degraded on those defaults, before and after a restart alike.
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;
    daemon.write_global_config(b"[files]\nexclude = \"*.log\"\n");
    assert_eq!(state(&daemon, &root).await.0, "degraded");

    let daemon = daemon.restart().await;

    assert_eq!(
        state(&daemon, &root).await,
        ("degraded".to_owned(), "config_invalid".to_owned())
    );
}

#[tokio::test]
async fn the_listener_is_on_by_default_and_reported_as_a_key_of_every_root() {
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;

    let configuration = configuration(&daemon, &root).await;

    let listener = key(&configuration, "ui.listener");
    assert_eq!(listener["value"], true);
    assert_eq!(listener["origin"]["level"], "default");
    assert_eq!(listener["restart_required"], false);
}

#[tokio::test]
async fn a_daemon_key_in_a_per_root_block_is_ignored_with_a_warning() {
    // A key describing the daemon has one value per process, so only the global file's own table sets
    // it: a block naming one Root, or a pattern of them, cannot.
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    let glob = format!("{}/*", project.parent().expect("a parent").display()).replace('\\', "/");
    daemon.write_global_config(
        format!(
            "[root.{root}.ui]\nlistener = false\n\n\
             [[roots]]\npath = \"{glob}\"\n[roots.ui]\nport = 9999\n"
        )
        .as_bytes(),
    );

    let configuration = configuration(&daemon, &root).await;

    assert_eq!(key(&configuration, "ui.listener")["value"], true);
    assert_eq!(key(&configuration, "ui.port")["origin"]["level"], "default");
    let ignored: Vec<&str> = configuration["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .filter(|warning| warning["code"] == "key_ignored")
        .map(|warning| warning["message"].as_str().expect("a message"))
        .collect();
    assert_eq!(ignored.len(), 2, "{configuration}");
    assert!(
        ignored
            .iter()
            .any(|message| message.contains("ui.listener")),
        "{ignored:?}"
    );
    assert!(
        ignored.iter().any(|message| message.contains("ui.port")),
        "{ignored:?}"
    );
    assert_eq!(configuration["errors"], serde_json::json!([]));
    watching(&daemon, &root).await;
}

#[tokio::test]
async fn a_root_file_setting_a_daemon_key_is_ignored_with_a_warning() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(&project, ".keepster.toml", b"[ui]\nlistener = false\n");

    let configuration = configuration(&daemon, &root).await;

    assert_eq!(key(&configuration, "ui.listener")["value"], true);
    assert_eq!(configuration["warnings"][0]["code"], "key_ignored");
    watching(&daemon, &root).await;
}

#[tokio::test]
async fn an_allowed_origin_that_is_a_wildcard_is_refused_as_invalid() {
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;
    daemon.write_global_config(b"[ui]\nallowed_origins = [\"*\"]\n");

    let configuration = configuration(&daemon, &root).await;

    assert_eq!(
        configuration["errors"][0]["code"], "config_invalid",
        "{configuration}"
    );
    assert!(
        configuration["errors"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("ui.allowed_origins")),
        "{configuration}"
    );
}

#[tokio::test]
async fn an_edit_to_a_restart_required_key_is_reported_until_the_daemon_restarts() {
    // Ticket 42's restart-required criterion, end to end, with its first key.
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;

    daemon.write_global_config(b"[ui]\nlistener = false\n");
    let edited = configuration(&daemon, &root).await;
    let listener = key(&edited, "ui.listener");
    assert_eq!(
        listener["value"], true,
        "the value in force is the one the daemon started with"
    );
    assert_eq!(listener["origin"]["level"], "default");
    assert_eq!(listener["restart_required"], true);

    let daemon = daemon.restart().await;

    let restarted = configuration(&daemon, &root).await;
    let listener = key(&restarted, "ui.listener");
    assert_eq!(listener["value"], false);
    assert_eq!(listener["origin"]["level"], "global");
    assert_eq!(listener["restart_required"], false);
}

#[tokio::test]
async fn the_log_keys_are_reported_at_their_defaults() {
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;

    let configuration = configuration(&daemon, &root).await;

    assert_eq!(key(&configuration, "log.level")["value"], "info");
    assert_eq!(key(&configuration, "log.format")["value"], "text");
    assert_eq!(
        key(&configuration, "log.ceiling")["value"],
        50 * 1024 * 1024
    );
    assert_eq!(
        key(&configuration, "log.ceiling")["origin"]["level"],
        "default"
    );
}

#[tokio::test]
async fn the_log_keys_describe_the_daemon_and_a_root_file_cannot_set_them() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(
        &project,
        ".keepster.toml",
        b"[log]\nformat = \"json\"\nceiling = \"1 GiB\"\n",
    );

    let configuration = configuration(&daemon, &root).await;

    assert_eq!(key(&configuration, "log.format")["value"], "text");
    let ignored = configuration["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .filter(|warning| warning["code"] == "key_ignored")
        .count();
    assert_eq!(ignored, 2, "{configuration}");
    watching(&daemon, &root).await;
}

#[tokio::test]
async fn a_log_format_or_level_this_build_does_not_know_is_refused() {
    let daemon = Harness::start().await;
    let (root, _) = registered(&daemon).await;

    for (written, key_name) in [
        (&b"[log]\nformat = \"xml\"\n"[..], "log.format"),
        (&b"[log]\nlevel = \"chatty\"\n"[..], "log.level"),
        (&b"[log]\nceiling = \"1 KiB\"\n"[..], "log.ceiling"),
    ] {
        daemon.write_global_config(written);
        let configuration = configuration(&daemon, &root).await;
        assert!(
            configuration["errors"][0]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(key_name)),
            "{key_name}: {configuration}"
        );
    }
}
