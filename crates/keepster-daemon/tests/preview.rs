//! A Capture Preview through `/v1` (ticket 57, ticket 20): what a Snapshot of a directory would hold,
//! streamed as one JSON object per line — for a directory that is not a Root, honouring its own
//! `.keepster.toml` and showing what it sets, and for a Root, without capturing, locking or triggering
//! anything; one at a time, and stopped when its reader goes away.

use std::path::Path;
use std::time::Duration;

use keepster_api::encode_path_query;
use keepster_cli::{ClientError, DaemonClient};
use keepster_testkit::{Harness, MANUAL};
use serde_json::{Value, json};

fn url(path: &Path, query: &str) -> String {
    format!(
        "/v1/preview?path={}{query}",
        encode_path_query(&path.to_string_lossy())
    )
}

/// Every record of a finished preview of `path`.
async fn records(daemon: &Harness, path: &Path, query: &str) -> Vec<Value> {
    let response = daemon.get(&url(path, query)).await;
    assert_eq!(
        response.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    assert_eq!(
        response.headers["content-type"], "application/x-ndjson",
        "one JSON object per line"
    );
    String::from_utf8_lossy(&response.body)
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect()
}

/// A directory holding `notes.txt`, `debug.tmp`, a `node_modules` tree and its own `.keepster.toml`
/// excluding `*.tmp`.
fn candidate(daemon: &Harness) -> std::path::PathBuf {
    let project = daemon.scratch("candidate");
    std::fs::create_dir_all(project.join("node_modules").join("lib")).expect("a directory");
    std::fs::write(project.join("notes.txt"), "kept").expect("a file");
    std::fs::write(project.join("debug.tmp"), "x".repeat(40)).expect("a file");
    std::fs::write(
        project.join("node_modules").join("lib").join("index.js"),
        "y".repeat(30),
    )
    .expect("a file");
    std::fs::write(
        project.join(".keepster.toml"),
        "[files]\nexclude = [\"*.tmp\", \"node_modules/\"]\n",
    )
    .expect("its own file");
    project
}

#[tokio::test]
async fn a_directory_that_is_no_root_is_previewed_under_its_own_file_and_shows_what_it_sets() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = candidate(&daemon);

    let records = records(&daemon, &project, "").await;

    let header = &records[0];
    assert_eq!(header["type"], "header", "{header}");
    assert!(header.get("root").is_none(), "not a Root: {header}");
    assert_eq!(
        header["config"]["sets"],
        json!([{ "key": "files.exclude", "values": ["*.tmp", "node_modules/"] }]),
        "what it sets, and to what: {header}"
    );
    let summary = records.last().expect("a summary");
    assert_eq!(summary["type"], "summary", "{summary}");
    assert_eq!(
        records.len(),
        2,
        "the summary alone by default: {records:?}"
    );
    assert_eq!(
        summary["captured"]["files"], 2,
        "notes.txt and .keepster.toml: {summary}"
    );
    assert_eq!(summary["excluded"]["files"], 1, "{summary}");
    assert_eq!(summary["excluded"]["bytes"], 40, "{summary}");
    assert_eq!(
        summary["excluded"]["pruned"], 1,
        "named, never entered: {summary}"
    );
    let tmp = summary["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .find(|group| group["rule"]["pattern"] == "*.tmp")
        .expect("grouped by the rule that decided");
    assert_eq!(
        tmp["rule"]["level"], "root",
        "the directory's own file: {tmp}"
    );
    assert_eq!(
        summary["largest_left_out"][0]["path"], "debug.tmp",
        "{summary}"
    );
    assert!(
        summary.get("store_growth").is_none() && !summary.to_string().contains("growth"),
        "no store growth is ever estimated: {summary}"
    );
}

#[tokio::test]
async fn the_full_depth_form_lists_every_path_and_only_lists_one_class() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = candidate(&daemon);

    let every = records(&daemon, &project, "&depth=infinity").await;
    let paths: Vec<&str> = every
        .iter()
        .filter(|record| record["type"] == "entry")
        .map(|record| record["path"].as_str().expect("a path"))
        .collect();
    assert!(
        paths.contains(&"node_modules/lib/index.js"),
        "entered to be listed: {paths:?}"
    );

    let excluded = records(&daemon, &project, "&only=excluded").await;
    let classes: Vec<&str> = excluded
        .iter()
        .filter(|record| record["type"] == "entry")
        .map(|record| record["class"].as_str().expect("a class"))
        .collect();
    assert!(!classes.is_empty());
    assert!(
        classes.iter().all(|class| *class == "excluded"),
        "{classes:?}"
    );
}

#[tokio::test]
async fn a_root_is_previewed_without_capturing_or_triggering_anything() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = candidate(&daemon);
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let previewed = records(&daemon, &project, "").await;

    assert_eq!(previewed[0]["root"], root.as_str(), "{}", previewed[0]);
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(
        timeline["snapshots"].as_array().map(Vec::len),
        Some(0),
        "nothing captured: {timeline}"
    );
    let state = daemon.get_json(&format!("/v1/roots/{root}")).await;
    assert_eq!(state["pending_paths"], 0, "no Trigger: {state}");
}

#[tokio::test]
async fn a_directory_inside_a_root_is_classified_by_the_roots_rules() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = candidate(&daemon);
    std::fs::create_dir_all(project.join("src")).expect("a directory");
    std::fs::write(project.join("src").join("main.rs"), "fn main() {}").expect("a file");
    std::fs::write(project.join("src").join("scratch.tmp"), "x").expect("a file");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let inner = records(&daemon, &project.join("src"), "&depth=infinity").await;

    assert_eq!(inner[0]["root"], root.as_str(), "{}", inner[0]);
    assert_eq!(inner[0]["within"], "src", "{}", inner[0]);
    assert!(inner[0].get("overlaps").is_none(), "{}", inner[0]);
    let mut classes: Vec<(&str, &str)> = inner
        .iter()
        .filter(|record| record["type"] == "entry")
        .map(|record| {
            (
                record["path"].as_str().expect("a path"),
                record["class"].as_str().expect("a class"),
            )
        })
        .collect();
    // In the order the platform lists them, which is no order.
    classes.sort_unstable();
    assert_eq!(
        classes,
        [("src/main.rs", "captured"), ("src/scratch.tmp", "excluded")],
        "the Root's paths, by the Root's file"
    );

    // A directory a capture of the Root never enters is that one answer.
    let pruned = records(&daemon, &project.join("node_modules"), "").await;
    let summary = pruned.last().expect("a summary");
    assert_eq!(summary["excluded"]["pruned"], 1, "{summary}");
    assert_eq!(summary["captured"]["files"], 0, "{summary}");
}

#[tokio::test]
async fn a_directory_holding_a_root_would_be_refused_and_never_enters_it() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let outer = daemon.scratch("outer");
    let inner = outer.join("inner");
    std::fs::create_dir_all(&inner).expect("a directory");
    std::fs::write(inner.join("held.txt"), "theirs").expect("a file");
    std::fs::write(outer.join("mine.txt"), "mine").expect("a file");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": inner }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    let previewed = records(&daemon, &outer, "&depth=infinity").await;

    assert_eq!(previewed[0]["overlaps"], root.as_str(), "{}", previewed[0]);
    assert!(previewed[0].get("root").is_none(), "{}", previewed[0]);
    let entries: Vec<&Value> = previewed
        .iter()
        .filter(|record| record["type"] == "entry")
        .collect();
    assert!(
        entries
            .iter()
            .any(|entry| entry["path"] == "inner" && entry["reason"] == "other_root"),
        "{entries:?}"
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry["path"] != "inner/held.txt"),
        "another Root is never entered: {entries:?}"
    );
}

#[tokio::test]
async fn a_long_walk_says_now_and_then_how_far_it_has_got() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let many = daemon.scratch("many");
    for at in 0..1500 {
        std::fs::write(many.join(format!("{at:04}.txt")), b"x").expect("a file");
    }

    let records = records(&daemon, &many, "").await;

    let progress: Vec<&Value> = records
        .iter()
        .filter(|record| record["type"] == "progress")
        .collect();
    assert_eq!(
        progress,
        [&json!({ "type": "progress", "walked": 1000 })],
        "with no entry listed, the walk still says it is alive"
    );
    assert_eq!(
        records.last().map(|record| &record["captured"]["files"]),
        Some(&json!(1500))
    );
}

#[tokio::test]
async fn a_malformed_request_is_refused_at_the_door() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = candidate(&daemon);

    let relative = daemon.get("/v1/preview?path=relative").await;
    assert_eq!(relative.status.as_u16(), 400);
    let only = daemon.get(&url(&project, "&only=everything")).await;
    assert_eq!(only.status.as_u16(), 400);
    let refused: Value = serde_json::from_slice(&only.body).expect("a problem");
    assert_eq!(refused["code"], "preview_invalid");
}

#[tokio::test]
async fn one_preview_walks_at_a_time_and_it_stops_when_its_reader_goes_away() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let big = daemon.scratch("big");
    // Lines enough to fill, many times over, every buffer between the walk and a reader that has stopped
    // reading — hyper's on both sides, the socket's or the named pipe's, the channel's: some 4 MB of them,
    // under names short enough for any platform's path limit.
    let padding = "n".repeat(130);
    for at in 0..20_000 {
        std::fs::write(big.join(format!("{padding}-{at:05}.txt")), b"x").expect("a file");
    }
    let client = DaemonClient::new(daemon.control_path());

    let mut reading = client
        .lines(&url(&big, "&depth=infinity"))
        .await
        .expect("a stream");
    let header: Value =
        serde_json::from_str(&reading.next().await.expect("a line").expect("the header"))
            .expect("JSON");
    assert_eq!(
        header["type"], "header",
        "the first line arrives before the walk ends"
    );

    match client.lines(&url(&big, "")).await {
        Err(ClientError::Refused(refused)) => {
            assert_eq!(refused.status.as_u16(), 409);
            let problem: Value = serde_json::from_slice(&refused.body).expect("a problem");
            assert_eq!(problem["code"], "preview_busy");
        }
        other => panic!("a second preview is refused while one walks: {other:?}"),
    }

    // The reader goes away; the walk stops, and the next preview walks.
    drop(reading);
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if daemon.get(&url(&big, "")).await.status.as_u16() == 200 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the first walk stopped with its reader");
}

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn the_cli_renders_the_preview_and_says_it_estimates_no_store_growth() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = candidate(&daemon);
    let directory = project.to_string_lossy().into_owned();

    let run = daemon
        .run_cli(KEEPSTER, &["preview", directory.as_str()])
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let printed = String::from_utf8_lossy(&run.stdout);
    assert!(printed.contains("Capture Preview of"), "{printed}");
    assert!(printed.contains("(not a Root)"), "{printed}");
    assert!(
        printed.contains("files.exclude = *.tmp, node_modules/"),
        "what its own file sets, and to what: {printed}"
    );
    assert!(printed.contains("captured:   2 files"), "{printed}");
    assert!(
        printed.contains("*.tmp (root"),
        "grouped by the rule: {printed}"
    );
    assert!(printed.contains("node_modules/ not entered"), "{printed}");
    assert!(
        printed.contains("Store growth is not estimated"),
        "{printed}"
    );

    let json = daemon
        .run_cli(KEEPSTER, &["preview", directory.as_str(), "--json"])
        .await;
    assert_eq!(json.exit_code, Some(0), "{}", json.stderr);
    let lines: Vec<Value> = String::from_utf8_lossy(&json.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).expect("the stream, verbatim"))
        .collect();
    assert_eq!(
        lines.first().map(|line| &line["type"]),
        Some(&json!("header"))
    );
    assert_eq!(
        lines.last().map(|line| &line["type"]),
        Some(&json!("summary"))
    );
}

#[tokio::test]
async fn with_no_directory_the_cli_previews_the_root_holding_the_working_directory() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = candidate(&daemon);
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let inside = project.join("node_modules").join("lib");

    let run = daemon
        .run_cli_in(&inside, KEEPSTER, &["preview", "--json"])
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let printed = String::from_utf8_lossy(&run.stdout);
    let header: Value =
        serde_json::from_str(printed.lines().next().expect("a header")).expect("JSON");
    assert_eq!(header["root"], root.as_str(), "{header}");
    assert!(
        header.get("within").is_none(),
        "the whole Root, as every command finds it: {header}"
    );
}

#[tokio::test]
async fn watch_dry_run_is_the_preview_and_watches_nothing() {
    let daemon = Harness::start_with_global_config(MANUAL).await;
    let project = candidate(&daemon);
    let directory = project.to_string_lossy().into_owned();

    let run = daemon
        .run_cli(KEEPSTER, &["watch", directory.as_str(), "--dry-run"])
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("Capture Preview of"),
        "{}",
        String::from_utf8_lossy(&run.stdout)
    );
    assert_eq!(
        daemon.get_json("/v1/roots").await,
        json!([]),
        "nothing registered"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn watching_a_directory_past_the_claim_asks_first_and_never_refuses() {
    // A sparse file costs no disk; its length is what a preview measures, and what the claim bounds.
    let daemon = Harness::start_with_global_config(
        format!(
            "{}\n[files]\nmax_size = \"20 GiB\"\n",
            String::from_utf8_lossy(MANUAL)
        )
        .as_bytes(),
    )
    .await;
    let project = daemon.scratch("huge");
    std::fs::File::create(project.join("disk.img"))
        .and_then(|file| file.set_len(11 * 1024 * 1024 * 1024))
        .expect("a sparse file");
    let directory = project.to_string_lossy().into_owned();

    // Not a terminal, so nothing can be asked: not watched, and said why.
    let refused = daemon
        .run_cli(KEEPSTER, &["watch", directory.as_str()])
        .await;
    assert_eq!(
        refused.exit_code,
        Some(5),
        "waiting on the person's word: {}",
        refused.stderr
    );
    assert!(refused.stderr.contains("--yes"), "{}", refused.stderr);
    assert!(
        String::from_utf8_lossy(&refused.stdout).contains("past what one Root is claimed to hold"),
        "{}",
        String::from_utf8_lossy(&refused.stdout)
    );
    assert_eq!(daemon.get_json("/v1/roots").await, json!([]));

    let watched = daemon
        .run_cli(KEEPSTER, &["watch", directory.as_str(), "--yes"])
        .await;
    assert_eq!(watched.exit_code, Some(0), "{}", watched.stderr);
    assert_eq!(
        daemon.get_json("/v1/roots").await.as_array().map(Vec::len),
        Some(1)
    );
}
