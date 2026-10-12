//! Search across Snapshots at the `/v1` seam (ticket 59): a path found by its name in every Root's history,
//! a deleted file included and said to be gone; and text found in every version a Root holds, streamed —
//! the cost first, a row per content version rather than per Snapshot, and a summary that tells finding
//! nothing from not looking.

use std::path::{Path, PathBuf};

use keepster_api::encode_path_query;
use keepster_testkit::{Harness, MANUAL};
use serde_json::{Value, json};

/// A daemon that captures only when asked, and withholds any file over 64 bytes.
async fn daemon() -> Harness {
    let config = format!(
        "{}\n[files]\nmax_size = \"64 B\"\n",
        String::from_utf8_lossy(MANUAL)
    );
    Harness::start_with_global_config(config.as_bytes()).await
}

fn write(directory: &Path, path: &str, content: &[u8]) {
    let file = directory.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, content).expect("a file");
}

async fn register(daemon: &Harness, name: &str) -> (String, PathBuf) {
    let directory = daemon.scratch(name);
    let id = daemon
        .post_json("/v1/roots", &json!({ "path": directory }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    (id, directory)
}

async fn capture(daemon: &Harness, root: &str) -> String {
    daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await["id"]
        .as_str()
        .expect("a Snapshot")
        .to_owned()
}

/// Two Roots: `project`, whose `old.txt` held the needle and was deleted, and whose `main.rs` gained one,
/// beside a binary and an over-cap file; and `notes`, whose `todo.txt` holds one.
async fn two_roots(daemon: &Harness) -> ((String, PathBuf, [String; 2]), (String, PathBuf)) {
    let (project, directory) = register(daemon, "project").await;
    write(&directory, "old.txt", b"the needle was here\n");
    write(&directory, "src/main.rs", b"fn main() {}\n");
    write(&directory, "logo.png", b"\x89PNG\0\0needle");
    write(&directory, "big.bin", &[b'z'; 100]);
    let first = capture(daemon, &project).await;
    std::fs::remove_file(directory.join("old.txt")).expect("deleted");
    write(&directory, "src/main.rs", b"fn main() {}\n// a needle\n");
    let second = capture(daemon, &project).await;

    let (notes, other) = register(daemon, "notes").await;
    write(&other, "todo.txt", b"buy a needle\n");
    capture(daemon, &notes).await;

    ((project, directory, [first, second]), (notes, other))
}

/// Every record of a finished content search.
async fn records(daemon: &Harness, query: &str) -> Vec<Value> {
    let response = daemon.get(&format!("/v1/search/content?{query}")).await;
    assert_eq!(
        response.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    assert_eq!(response.headers["content-type"], "application/x-ndjson");
    String::from_utf8_lossy(&response.body)
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect()
}

#[tokio::test]
async fn a_deleted_file_is_found_by_its_name_across_every_root_and_said_to_be_gone() {
    let daemon = daemon().await;
    let ((project, _, [first, second]), (notes, _)) = two_roots(&daemon).await;

    let found = daemon.get_json("/v1/search/paths?q=old").await;
    assert_eq!(found["truncated"], false, "{found}");
    assert_eq!(
        found["hits"],
        json!([{
            "root": project,
            "path": "old.txt",
            "snapshots": 2,
            "first": first,
            "last": second,
            "exists": false,
        }]),
        "found, and already gone"
    );

    let everywhere = daemon.get_json("/v1/search/paths?q=.txt").await;
    let roots: Vec<&str> = everywhere["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|hit| hit["root"].as_str().expect("a root"))
        .collect();
    assert!(
        roots.contains(&project.as_str()) && roots.contains(&notes.as_str()),
        "every Root by default: {everywhere}"
    );

    let one = daemon
        .get_json(&format!("/v1/search/paths?q=.txt&roots={notes}"))
        .await;
    assert_eq!(one["hits"].as_array().map(Vec::len), Some(1), "{one}");

    let pattern = daemon
        .get_json(&format!(
            "/v1/search/paths?q={}&glob=true",
            encode_path_query("src/*.rs")
        ))
        .await;
    assert_eq!(pattern["hits"][0]["path"], "src/main.rs", "{pattern}");
    assert_eq!(pattern["hits"][0]["exists"], true);

    for limit in ["0", "many"] {
        let refused = daemon
            .get(&format!("/v1/search/paths?q=old&limit={limit}"))
            .await;
        assert_eq!(refused.status.as_u16(), 400);
        let problem: Value = serde_json::from_slice(&refused.body).expect("a problem");
        assert_eq!(problem["code"], "limit_invalid", "{problem}");
    }

    let empty = daemon.get("/v1/search/paths?q=").await;
    assert_eq!(empty.status.as_u16(), 400);
    let refused: Value = serde_json::from_slice(&empty.body).expect("a problem");
    assert_eq!(refused["code"], "query_invalid");
}

#[tokio::test]
async fn content_is_found_per_version_with_the_cost_first_and_a_summary_last() {
    let daemon = daemon().await;
    let ((project, _, [first, second]), _) = two_roots(&daemon).await;

    let records = records(&daemon, &format!("q=needle&roots={project}")).await;

    assert_eq!(records[0]["type"], "plan", "{}", records[0]);
    assert_eq!(records[0]["roots"][0]["root"], project.as_str());
    assert_eq!(records[0]["roots"][0]["versions"], 4, "{}", records[0]);
    let mut matches: Vec<(String, String, String, u64)> = records
        .iter()
        .filter(|record| record["type"] == "match")
        .map(|record| {
            (
                record["path"].as_str().expect("a path").to_owned(),
                record["first"].as_str().expect("a first").to_owned(),
                record["last"].as_str().expect("a last").to_owned(),
                record["line"].as_u64().expect("a line"),
            )
        })
        .collect();
    matches.sort();
    assert_eq!(
        matches,
        [
            ("old.txt".to_owned(), first.clone(), first.clone(), 1),
            ("src/main.rs".to_owned(), second.clone(), second.clone(), 2),
        ],
        "a row per content version, never per Snapshot"
    );
    let withheld: Vec<&Value> = records
        .iter()
        .filter(|record| record["type"] == "withheld")
        .collect();
    assert_eq!(withheld.len(), 1, "{records:?}");
    assert_eq!(withheld[0]["path"], "big.bin");
    assert_eq!(withheld[0]["reason"], "too_large");
    let summary = records.last().expect("a summary");
    assert_eq!(summary["type"], "summary", "{summary}");
    assert_eq!(summary["binaries_skipped"], 1, "the image: {summary}");
    assert_eq!(summary["versions_scanned"], 3, "{summary}");
    assert_eq!(summary["withheld"], 1, "{summary}");
}

#[tokio::test]
async fn content_search_takes_one_root_unless_every_one_is_asked_for() {
    let daemon = daemon().await;
    let ((project, _, _), (notes, _)) = two_roots(&daemon).await;

    let unnamed = daemon.get("/v1/search/content?q=needle").await;
    assert_eq!(unnamed.status.as_u16(), 400);
    let refused: Value = serde_json::from_slice(&unnamed.body).expect("a problem");
    assert_eq!(refused["code"], "search_invalid");

    let every = records(&daemon, "q=needle&all=true").await;
    let planned: Vec<&str> = every[0]["roots"]
        .as_array()
        .expect("roots")
        .iter()
        .map(|cost| cost["root"].as_str().expect("a root"))
        .collect();
    assert!(
        planned.contains(&project.as_str()) && planned.contains(&notes.as_str()),
        "the cost of each Root, said first: {}",
        every[0]
    );
    assert!(
        every
            .iter()
            .any(|record| record["type"] == "match" && record["path"] == "todo.txt")
    );

    let beneath = records(&daemon, &format!("q=needle&roots={project}&path=src")).await;
    let paths: Vec<&str> = beneath
        .iter()
        .filter(|record| record["type"] == "match")
        .map(|record| record["path"].as_str().expect("a path"))
        .collect();
    assert_eq!(paths, ["src/main.rs"]);

    let broken = daemon
        .get(&format!(
            "/v1/search/content?q={}&regex=true&roots={project}",
            encode_path_query("(unclosed")
        ))
        .await;
    assert_eq!(broken.status.as_u16(), 400);
}

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn the_cli_finds_a_path_and_greps_the_history() {
    let daemon = daemon().await;
    let ((_, directory, _), _) = two_roots(&daemon).await;

    let found = daemon.run_cli(KEEPSTER, &["find", "old"]).await;
    assert_eq!(found.exit_code, Some(0), "{}", found.stderr);
    let printed = String::from_utf8_lossy(&found.stdout);
    assert!(printed.contains("old.txt"), "{printed}");
    assert!(
        printed.contains("gone from the newest Snapshot"),
        "{printed}"
    );

    let pattern = daemon
        .run_cli(KEEPSTER, &["find", "--glob", "src/*.rs"])
        .await;
    assert_eq!(pattern.exit_code, Some(0), "{}", pattern.stderr);
    let printed = String::from_utf8_lossy(&pattern.stdout);
    // The tail only: the daemon may spell the Root's own directory as the platform resolves it.
    let tail = Path::new("project").join("src").join("main.rs");
    assert!(
        printed.contains(&tail.display().to_string()),
        "where it is on this machine: {printed}"
    );

    let grepped = daemon
        .run_cli_in(&directory, KEEPSTER, &["grep", "needle"])
        .await;
    assert_eq!(grepped.exit_code, Some(0), "{}", grepped.stderr);
    let printed = String::from_utf8_lossy(&grepped.stdout);
    assert!(
        printed.contains("old.txt:1: the needle was here"),
        "{printed}"
    );
    assert!(
        grepped.stderr.contains("big.bin was not searched"),
        "{}",
        grepped.stderr
    );
    assert!(
        grepped.stderr.contains("1 binary skipped"),
        "{}",
        grepped.stderr
    );

    assert!(
        printed.contains(&format!(
            "{}:2: // a needle",
            Path::new("src").join("main.rs").display()
        )),
        "Root-relative, spelled as this machine spells a path: {printed}"
    );

    let everywhere = daemon
        .run_cli_in(&directory, KEEPSTER, &["grep", "--all-roots", "NEEDLE"])
        .await;
    assert_eq!(everywhere.exit_code, Some(0), "{}", everywhere.stderr);
    assert!(
        everywhere.stdout.is_empty(),
        "a capital asks for case: {}",
        String::from_utf8_lossy(&everywhere.stdout)
    );
    let everywhere = daemon
        .run_cli_in(&directory, KEEPSTER, &["grep", "--all-roots", "buy a"])
        .await;
    let printed = String::from_utf8_lossy(&everywhere.stdout);
    assert!(
        printed.contains(&format!(
            "{}:1: buy a needle",
            Path::new("notes").join("todo.txt").display()
        )),
        "every Root, each path where it is: {printed}"
    );

    let json = daemon
        .run_cli_in(&directory, KEEPSTER, &["grep", "needle", "--json"])
        .await;
    assert_eq!(json.exit_code, Some(0), "{}", json.stderr);
    let last = String::from_utf8_lossy(&json.stdout)
        .lines()
        .last()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON"))
        .expect("a line");
    assert_eq!(last["type"], "summary");
}
