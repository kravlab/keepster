//! What the daemon hands a diagnostics bundle (ticket 63, ADR 0013): every section the bundle carries,
//! on the control channel alone, and never a byte of a file from history.

use std::time::Duration;

use keepster_testkit::Harness;
use serde_json::{Value, json};

use crate::jobs::captured;

/// What a captured file holds, chosen so that it appears nowhere else: not in a path, a name or a
/// message — so finding it anywhere in an answer means a file's content got in.
const CANARY: &str = "keepster-canary-0f3e9b7c-content-of-a-captured-file";

async fn diagnostics(daemon: &Harness) -> Value {
    let answer = daemon.get("/v1/diagnostics").await;
    assert_eq!(
        answer.status,
        200,
        "{}",
        String::from_utf8_lossy(&answer.body)
    );
    serde_json::from_slice(&answer.body).expect("JSON")
}

/// Wait until the job `job` has ended, and return how.
async fn ended(daemon: &Harness, job: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let job = daemon.get_json(&format!("/v1/jobs/{job}")).await;
            if job["state"] == "succeeded" || job["state"] == "failed" {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the job ends within the timeout")
}

/// Run a Maintenance Pass over `root` as a person asks for one, and wait for it to end.
async fn maintain(daemon: &Harness, root: &str) -> Value {
    let accepted = daemon
        .post_json(&format!("/v1/roots/{root}/maintenance"), &json!({}))
        .await;
    ended(daemon, accepted["job_id"].as_str().expect("a job id")).await
}

#[tokio::test]
async fn every_section_a_bundle_carries_is_served() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let root = captured(&daemon).await;

    let found = diagnostics(&daemon).await;

    assert_eq!(
        found["status"]["daemon"]["api_version"], "v1",
        "status, verbatim: {found}"
    );
    assert_eq!(found["status"]["roots"][0]["id"], root.as_str());

    let environment = &found["environment"];
    assert_eq!(environment["os"], std::env::consts::OS, "{environment}");
    assert_eq!(environment["arch"], std::env::consts::ARCH);
    assert_eq!(environment["filesystems"][0]["root"], root.as_str());
    assert_eq!(
        environment["filesystems"][0]["filesystem"], "testfs",
        "the filesystem under the Root, as the platform names it: {environment}"
    );
    assert!(
        environment["watches"]["in_use"].is_u64(),
        "the watches every Root holds together: {environment}"
    );
    assert_eq!(
        environment["git"], found["status"]["daemon"]["git"],
        "the git discovery result"
    );
    assert_eq!(environment["maintenance"], json!([]), "no pass has run yet");

    let configuration = &found["configuration"];
    let global_keys: Vec<&str> = configuration["global"]["keys"]
        .as_array()
        .expect("the global keys")
        .iter()
        .filter_map(|key| key["key"].as_str())
        .collect();
    assert!(
        global_keys.contains(&"log.format") && global_keys.contains(&"triggers.active"),
        "every key of the schema at the global level: {global_keys:?}"
    );
    let active = configuration["global"]["keys"]
        .as_array()
        .expect("the global keys")
        .iter()
        .find(|key| key["key"] == "triggers.active")
        .expect("the key");
    assert_eq!(
        active["origin"]["level"], "global",
        "with the level that set it: {active}"
    );
    assert_eq!(
        configuration["roots"][0]["root"],
        root.as_str(),
        "{configuration}"
    );
    assert!(configuration["roots"][0]["configuration"]["keys"].is_array());

    assert_eq!(found["registry"][0]["id"], root.as_str(), "{found}");
    assert_eq!(found["registry"][0]["standing"], "watched");
    assert!(found["registry"][0]["path"].is_string());

    let topology = &found["topology"][0];
    assert_eq!(topology["root"], root.as_str(), "{topology}");
    assert_eq!(topology["store"]["segments"][0]["snapshots"], 1);
    assert!(topology["store"]["segments"][0]["newest"]["taken_at"].is_string());
    assert!(topology["store"]["history_bytes"].as_u64() > Some(0));

    let watermarks = &found["watermarks"][0];
    assert_eq!(watermarks["root"], root.as_str(), "{watermarks}");
    let source = &watermarks["sources"][0];
    assert!(
        source["tip"].is_string() && source["indexed"] == source["tip"],
        "the index caught up with the Segment it indexes: {watermarks}"
    );

    assert_eq!(
        found["log"]["format"], "text",
        "the format the log is written in: {found}"
    );
    assert_eq!(
        found["log"]["files"],
        json!([]),
        "a daemon in a test process writes no file"
    );
}

#[tokio::test]
async fn no_byte_of_a_file_from_history_is_ever_served() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.txt"), CANARY).expect("a file");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await;

    let answer = daemon.get("/v1/diagnostics").await;

    assert_eq!(answer.status, 200);
    assert!(
        !String::from_utf8_lossy(&answer.body).contains(CANARY),
        "the content of a captured file is in the diagnostics"
    );
}

#[tokio::test]
async fn the_diagnostics_are_handed_out_on_the_control_channel_alone() {
    let daemon = Harness::start().await;
    let token = std::fs::read_to_string(daemon.token_path()).expect("the token");
    let bearer = format!("Bearer {}", token.trim());
    let host = daemon.loopback().expect("listening").to_string();

    let answered = daemon
        .loopback_request(
            "GET",
            "/v1/diagnostics",
            &[("host", &host), ("authorization", &bearer)],
        )
        .await;

    assert_eq!(
        answered.status, 404,
        "the network surface serves no diagnostics, even to the token"
    );
}

#[tokio::test]
async fn a_root_s_last_maintenance_pass_is_reported_with_how_it_ended() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let root = captured(&daemon).await;

    let job = maintain(&daemon, &root).await;

    assert_eq!(job["state"], "succeeded", "{job}");
    let passes = &diagnostics(&daemon).await["environment"]["maintenance"];
    assert_eq!(passes[0]["root"], root.as_str(), "{passes}");
    assert_eq!(passes[0]["outcome"], "succeeded", "{passes}");
    assert!(passes[0]["finished_at"].is_string(), "{passes}");
    assert!(passes[0]["reclaimed_bytes"].is_u64(), "{passes}");
    assert!(passes[0].get("stderr").is_none(), "{passes}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_maintenance_pass_is_reported_with_what_git_wrote() {
    let daemon = crate::jobs::whose_git_fails().await;
    let root = captured(&daemon).await;

    let job = maintain(&daemon, &root).await;

    assert_eq!(job["state"], "failed", "{job}");
    let passes = &diagnostics(&daemon).await["environment"]["maintenance"];
    assert_eq!(passes[0]["root"], root.as_str(), "{passes}");
    assert_eq!(passes[0]["outcome"], "failed", "{passes}");
    assert!(
        passes[0]["stderr"]
            .as_str()
            .is_some_and(|stderr| stderr.contains("the disk said no")),
        "the last pass's standard error, word for word: {passes}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_pass_whose_git_fails_its_check_is_reported_git_unavailable() {
    let daemon = crate::jobs::whose_git_fails().await;
    let root = captured(&daemon).await;
    // Replaced after it was found: the check right before its first pass finds it too old.
    crate::jobs::install_script(
        &crate::jobs::fake_git(&daemon),
        "#!/bin/sh\necho 'git version 2.31.8'\nexit 0\n",
    );

    let job = maintain(&daemon, &root).await;

    assert_eq!(job["state"], "failed", "{job}");
    let passes = &diagnostics(&daemon).await["environment"]["maintenance"];
    assert_eq!(passes[0]["root"], root.as_str(), "{passes}");
    assert_eq!(passes[0]["outcome"], "git_unavailable", "{passes}");
    assert!(
        passes[0]["error"]
            .as_str()
            .is_some_and(|error| error.contains("2.31.8")),
        "why it could not run: {passes}"
    );
}

#[tokio::test]
async fn a_pass_over_a_history_that_cannot_be_opened_is_reported_history_unreadable() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let root = captured(&daemon).await;
    std::fs::write(
        daemon.store_dir(&root).join("history.git").join("config"),
        b"[core\n",
    )
    .expect("the store's configuration, broken");

    let job = maintain(&daemon, &root).await;

    assert_eq!(job["state"], "failed", "{job}");
    let found = diagnostics(&daemon).await;
    let passes = &found["environment"]["maintenance"];
    assert_eq!(passes[0]["outcome"], "history_unreadable", "{passes}");
    assert!(passes[0]["error"].is_string(), "{passes}");
    assert!(passes[0].get("stderr").is_none(), "git never ran: {passes}");
    assert!(
        found["topology"][0]["error"].is_string() && found["topology"][0].get("store").is_none(),
        "and the store's topology says why it is not there: {}",
        found["topology"]
    );
}

#[tokio::test]
async fn a_daemon_level_key_edited_since_the_start_is_shown_as_started_and_awaiting_a_restart() {
    let daemon = Harness::start().await;
    daemon.write_global_config(b"[ui]\nlistener = false\n");

    let found = diagnostics(&daemon).await;

    let listener = found["configuration"]["global"]["keys"]
        .as_array()
        .expect("the global keys")
        .iter()
        .find(|key| key["key"] == "ui.listener")
        .expect("the key")
        .clone();
    assert_eq!(
        listener["value"], true,
        "the value in force is the one the daemon started with: {listener}"
    );
    assert_eq!(listener["restart_required"], true, "{listener}");
}

#[tokio::test]
async fn the_watches_a_watched_root_holds_are_counted() {
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    std::fs::create_dir_all(project.join("src")).expect("a directory");
    daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await;

    // Zero until the Root's watcher is armed, and then every directory it watches.
    tokio::time::timeout(Duration::from_secs(30), async {
        while diagnostics(&daemon).await["environment"]["watches"]["in_use"]
            .as_u64()
            .expect("a count")
            == 0
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the watches the Root holds are counted once its watcher is armed");
}
