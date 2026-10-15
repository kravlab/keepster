//! Export at the `/v1` seam (ticket 58, ticket 19), against a real store: a Snapshot's versions written
//! by the daemon into a directory the client names — planned at once, written as a job — and a selection
//! handed to a browser instead, its size known before the click.

use std::path::{Path, PathBuf};
use std::time::Duration;

use keepster_cli::Response;
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

/// A Root holding `notes.txt`, `docs/guide.md` and an over-cap `big.bin`, and its Snapshot of them.
async fn captured(daemon: &Harness) -> (String, PathBuf, String) {
    let project = daemon.scratch("project");
    write(&project, "notes.txt", b"kept\n");
    write(&project, "docs/guide.md", b"# Guide\n");
    write(&project, "big.bin", &[b'z'; 100]);
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let snapshot = daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await["id"]
        .as_str()
        .expect("a Snapshot")
        .to_owned();

    (root, project, snapshot)
}

fn write(directory: &Path, path: &str, content: &[u8]) {
    let file = directory.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, content).expect("a file");
}

fn body(response: &Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

async fn export(daemon: &Harness, root: &str, request: Value) -> Response {
    daemon
        .post(&format!("/v1/roots/{root}/export"), &request)
        .await
}

/// Each file's outcome, as `(path, outcome)`.
fn outcomes(paths: &Value) -> Vec<(String, String)> {
    paths
        .as_array()
        .expect("paths")
        .iter()
        .map(|path| {
            (
                path["path"].as_str().expect("a path").to_owned(),
                path["outcome"].as_str().expect("an outcome").to_owned(),
            )
        })
        .collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected
        .iter()
        .map(|(path, outcome)| ((*path).to_owned(), (*outcome).to_owned()))
        .collect()
}

/// The job an accepted export started, once it has ended.
async fn finished(daemon: &Harness, accepted: &Response) -> Value {
    assert_eq!(
        accepted.status.as_u16(),
        202,
        "{}",
        String::from_utf8_lossy(&accepted.body)
    );
    let job = body(accepted)["job_id"].as_str().expect("a job").to_owned();
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let ended = daemon.get_json(&format!("/v1/jobs/{job}")).await;
            if ended["state"] == "succeeded" || ended["state"] == "failed" {
                break ended;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the job ends")
}

#[tokio::test]
async fn a_dry_run_says_what_would_happen_and_how_much_and_writes_nothing() {
    let daemon = daemon().await;
    let (root, _, snapshot) = captured(&daemon).await;
    let dest = daemon.scratch("exports").join("old");

    let answer = export(
        &daemon,
        &root,
        json!({ "from": snapshot, "dest": dest, "dry_run": true }),
    )
    .await;

    assert_eq!(answer.status.as_u16(), 200);
    let plan = body(&answer);
    assert_eq!(plan["files"], 2, "{plan}");
    assert_eq!(plan["bytes"], 13, "{plan}");
    assert_eq!(
        outcomes(&plan["paths"]),
        pairs(&[
            ("big.bin", "skipped"),
            ("docs/guide.md", "created"),
            ("notes.txt", "created"),
        ])
    );
    assert_eq!(plan["paths"][0]["issues"][0]["reason"], "withheld");
    assert_eq!(plan["paths"][0]["issues"][0]["why"], "too_large");
    assert!(!dest.exists(), "a dry run makes nothing");
}

#[tokio::test]
async fn an_export_is_a_job_that_writes_every_version_and_leaves_withheld_paths_out() {
    let daemon = daemon().await;
    let (root, project, snapshot) = captured(&daemon).await;
    write(&project, "notes.txt", b"edited since\n");
    let dest = daemon.scratch("exports").join("old");

    let accepted = export(&daemon, &root, json!({ "from": snapshot, "dest": dest })).await;
    let ended = finished(&daemon, &accepted).await;

    assert_eq!(ended["kind"], "export", "{ended}");
    assert_eq!(ended["state"], "succeeded", "{ended}");
    assert_eq!(
        outcomes(&ended["result"]["paths"]),
        pairs(&[
            ("big.bin", "skipped"),
            ("docs/guide.md", "created"),
            ("notes.txt", "created"),
        ])
    );
    assert_eq!(
        std::fs::read(dest.join("notes.txt")).expect("exported"),
        b"kept\n"
    );
    assert_eq!(
        std::fs::read(dest.join("docs").join("guide.md")).expect("exported"),
        b"# Guide\n"
    );
    assert!(
        !dest.join("big.bin").exists(),
        "absent, never an empty file standing in for it"
    );
    assert_eq!(
        std::fs::read(project.join("notes.txt")).expect("untouched"),
        b"edited since\n",
        "the Working Tree is untouched"
    );
}

#[tokio::test]
async fn overwriting_is_off_until_confirmed() {
    let daemon = daemon().await;
    let (root, _, snapshot) = captured(&daemon).await;
    let dest = daemon.scratch("exports");
    write(&dest, "notes.txt", b"mine\n");

    let refused = export(
        &daemon,
        &root,
        json!({ "from": snapshot, "dest": dest, "paths": ["notes.txt"] }),
    )
    .await;
    assert_eq!(refused.status.as_u16(), 400);
    let problem = body(&refused);
    assert_eq!(problem["code"], "confirmation_required", "{problem}");
    assert_eq!(problem["paths"][0]["issues"][0]["reason"], "overwrite");
    assert_eq!(problem["paths"][0]["issues"][0]["confirm"], "overwrite");
    assert_eq!(
        std::fs::read(dest.join("notes.txt")).expect("kept"),
        b"mine\n"
    );

    let accepted = export(
        &daemon,
        &root,
        json!({
            "from": snapshot,
            "dest": dest,
            "paths": ["notes.txt"],
            "confirm": ["overwrite"],
        }),
    )
    .await;
    let ended = finished(&daemon, &accepted).await;
    assert_eq!(
        outcomes(&ended["result"]["paths"]),
        pairs(&[("notes.txt", "restored")])
    );
    assert!(
        ended["result"].get("safety_snapshot").is_none(),
        "outside a Root there is nothing to take one of: {ended}"
    );
    assert_eq!(
        std::fs::read(dest.join("notes.txt")).expect("overwritten"),
        b"kept\n"
    );
}

#[tokio::test]
async fn into_a_root_is_confirmed_and_overwriting_there_takes_its_safety_snapshot() {
    let daemon = daemon().await;
    let (root, project, snapshot) = captured(&daemon).await;
    let dest = project.join("old");
    let request = |confirm: Value| json!({ "from": snapshot, "dest": dest, "paths": ["notes.txt"], "confirm": confirm });

    let refused = export(&daemon, &root, request(json!([]))).await;
    assert_eq!(refused.status.as_u16(), 400);
    assert_eq!(
        body(&refused)["paths"][0]["issues"][0]["reason"],
        "dest_in_root"
    );

    let first = finished(
        &daemon,
        &export(&daemon, &root, request(json!(["dest_in_root"]))).await,
    )
    .await;
    assert_eq!(
        outcomes(&first["result"]["paths"]),
        pairs(&[("notes.txt", "created")])
    );
    assert!(
        first["result"].get("safety_snapshot").is_none(),
        "nothing was overwritten: {first}"
    );

    // An edit made since, which only the Safety Snapshot can bring back once overwritten.
    write(&dest, "notes.txt", b"an edit in the old copy\n");
    let again = finished(
        &daemon,
        &export(
            &daemon,
            &root,
            request(json!(["dest_in_root", "overwrite"])),
        )
        .await,
    )
    .await;
    let safety = again["result"]["safety_snapshot"]
        .as_str()
        .expect("a Safety Snapshot")
        .to_owned();
    let held = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{safety}/content?path=old/notes.txt"
        ))
        .await;
    assert_eq!(
        held.body, b"an edit in the old copy\n",
        "what was overwritten can be restored"
    );
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(timeline["snapshots"][0]["trigger"], "export", "{timeline}");
}

#[tokio::test]
async fn a_destination_inside_a_history_store_or_not_a_directory_is_refused() {
    let daemon = daemon().await;
    let (root, _, snapshot) = captured(&daemon).await;

    let in_store = export(
        &daemon,
        &root,
        json!({ "from": snapshot, "dest": daemon.store_dir(&root).join("out") }),
    )
    .await;
    assert_eq!(in_store.status.as_u16(), 400);
    assert_eq!(body(&in_store)["code"], "dest_in_store");

    let file = daemon.scratch("exports").join("a-file");
    std::fs::write(&file, b"not a directory").expect("a file");
    let unwritable = export(&daemon, &root, json!({ "from": snapshot, "dest": file })).await;
    assert_eq!(unwritable.status.as_u16(), 409);
    assert_eq!(body(&unwritable)["code"], "dest_unwritable");

    let relative = export(&daemon, &root, json!({ "from": snapshot, "dest": "old" })).await;
    assert_eq!(relative.status.as_u16(), 400);
    assert_eq!(body(&relative)["code"], "path_invalid");
}

#[tokio::test]
async fn a_selection_s_size_is_known_before_its_archive_is_downloaded() {
    let daemon = daemon().await;
    let (root, _, snapshot) = captured(&daemon).await;

    let size = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots/{snapshot}/stat"))
        .await;
    assert_eq!(size["files"], 2, "{size}");
    assert_eq!(size["bytes"], 13, "{size}");
    assert_eq!(size["withheld"][0]["path"], "big.bin", "{size}");
    assert_eq!(size["withheld"][0]["reason"], "too_large", "{size}");

    let archive = daemon
        .get(&format!("/v1/roots/{root}/snapshots/{snapshot}/archive"))
        .await;
    assert_eq!(archive.status.as_u16(), 200);
    let header = |name: &str| {
        archive
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(header("content-type"), "application/zip");
    assert_eq!(
        header("content-length"),
        size["archive_bytes"].to_string(),
        "the size shown is the size sent"
    );
    assert_eq!(
        archive.body.len() as u64,
        size["archive_bytes"].as_u64().expect("a size")
    );
    assert!(
        header("content-disposition").starts_with("attachment;"),
        "{}",
        header("content-disposition")
    );
    assert!(archive.body.starts_with(b"PK\x03\x04"), "a zip");
    let held = |needle: &[u8]| {
        archive
            .body
            .windows(needle.len())
            .any(|window| window == needle)
    };
    assert!(held(b"kept\n") && held(b"# Guide\n"));
    assert!(
        !held(b"big.bin"),
        "a Withheld path is absent from the archive too"
    );

    let one = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/stat?path=docs"
        ))
        .await;
    assert_eq!(
        (one["files"].as_u64(), one["bytes"].as_u64()),
        (Some(1), Some(8))
    );
}

#[tokio::test]
async fn one_file_downloads_as_itself_and_a_patch_downloads_as_a_file() {
    let daemon = daemon().await;
    let (root, project, snapshot) = captured(&daemon).await;

    let file = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/content?path=docs/guide.md&download=1"
        ))
        .await;
    assert_eq!(file.body, b"# Guide\n");
    let disposition = file
        .headers
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        disposition.starts_with("attachment;") && disposition.contains("filename=\"guide.md\""),
        "{disposition}"
    );
    let shown = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/content?path=docs/guide.md"
        ))
        .await;
    assert!(shown.headers.get("content-disposition").is_none());

    write(&project, "notes.txt", b"changed\n");
    let later = daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await["id"]
        .as_str()
        .expect("a Snapshot")
        .to_owned();
    let patch = daemon
        .get(&format!(
            "/v1/roots/{root}/diff?from={snapshot}&to={later}&format=unified"
        ))
        .await;
    assert!(
        patch
            .headers
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("attachment;")),
        "saved byte for byte, not shown"
    );
}

#[tokio::test]
async fn a_directory_picker_lists_directories_only() {
    let daemon = daemon().await;
    let place = daemon.scratch("picker");
    std::fs::create_dir_all(place.join("photos")).expect("a directory");
    std::fs::create_dir_all(place.join(".cache")).expect("a hidden directory");
    std::fs::write(place.join("notes.txt"), b"a file").expect("a file");

    let listing = daemon
        .get_json(&format!(
            "/v1/fs?path={}",
            keepster_api::encode_path_query(place.to_string_lossy().as_bytes())
        ))
        .await;

    let names: Vec<(&str, bool)> = listing["directories"]
        .as_array()
        .expect("directories")
        .iter()
        .map(|directory| {
            (
                directory["name"].as_str().expect("a name"),
                directory["hidden"].as_bool().expect("hidden or not"),
            )
        })
        .collect();
    assert_eq!(names, [(".cache", true), ("photos", false)], "{listing}");
    assert_eq!(listing["directories"][1]["readable"], true);
    assert_eq!(
        listing["directories"][1]["path"],
        place.join("photos").to_string_lossy().as_ref()
    );
    assert!(listing["parent"].is_string(), "{listing}");

    let file = daemon
        .get(&format!(
            "/v1/fs?path={}",
            keepster_api::encode_path_query(place.join("notes.txt").to_string_lossy().as_bytes())
        ))
        .await;
    assert_eq!(file.status.as_u16(), 409);
    let missing = daemon
        .get(&format!(
            "/v1/fs?path={}",
            keepster_api::encode_path_query(place.join("nowhere").to_string_lossy().as_bytes())
        ))
        .await;
    assert_eq!(missing.status.as_u16(), 404);
    let relative = daemon.get("/v1/fs?path=relative").await;
    assert_eq!(relative.status.as_u16(), 400);
}

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn the_cli_exports_follows_the_job_and_asks_before_overwriting() {
    let daemon = daemon().await;
    let (_, project, _) = captured(&daemon).await;
    let dest = daemon.scratch("exports").join("old");
    let to = dest.to_string_lossy().into_owned();

    let planned = daemon
        .run_cli_in(&project, KEEPSTER, &["export", "--to", &to, "--dry-run"])
        .await;
    assert_eq!(planned.exit_code, Some(0), "{}", planned.stderr);
    let printed = String::from_utf8_lossy(&planned.stdout);
    assert!(printed.contains("would create  notes.txt"), "{printed}");
    assert!(
        printed.contains("would skip  big.bin  (withheld: too_large)"),
        "{printed}"
    );
    assert!(!dest.exists(), "a dry run makes nothing");

    let exported = daemon
        .run_cli_in(&project, KEEPSTER, &["export", "--to", &to])
        .await;
    assert_eq!(exported.exit_code, Some(0), "{}", exported.stderr);
    assert!(
        String::from_utf8_lossy(&exported.stdout).contains("created  docs/guide.md"),
        "{}",
        String::from_utf8_lossy(&exported.stdout)
    );
    assert_eq!(
        std::fs::read(dest.join("notes.txt")).expect("exported"),
        b"kept\n"
    );

    // Not a terminal, so nothing can be asked: refused, and said how to agree.
    write(&dest, "notes.txt", b"mine\n");
    let refused = daemon
        .run_cli_in(&project, KEEPSTER, &["export", "notes.txt", "--to", &to])
        .await;
    assert_eq!(refused.exit_code, Some(5), "{}", refused.stderr);
    assert!(refused.stderr.contains("--overwrite"), "{}", refused.stderr);
    assert_eq!(
        std::fs::read(dest.join("notes.txt")).expect("kept"),
        b"mine\n"
    );

    let overwritten = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &["export", "notes.txt", "--to", &to, "--overwrite"],
        )
        .await;
    assert_eq!(overwritten.exit_code, Some(0), "{}", overwritten.stderr);
    assert_eq!(
        std::fs::read(dest.join("notes.txt")).expect("overwritten"),
        b"kept\n"
    );

    let detached = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &["export", "--to", &to, "--force", "--detach"],
        )
        .await;
    assert_eq!(detached.exit_code, Some(0), "{}", detached.stderr);
    let job = String::from_utf8_lossy(&detached.stdout).trim().to_owned();
    assert_eq!(job.len(), 26, "the job's identity alone: {job}");
    // Waited for, so nothing still writes into the directory when the test's is removed.
    let ended = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let ended = daemon.get_json(&format!("/v1/jobs/{job}")).await;
            if ended["state"] == "succeeded" || ended["state"] == "failed" {
                break ended;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the job ends");
    assert_eq!(ended["state"], "succeeded", "{ended}");
}

#[tokio::test]
async fn the_cli_writes_an_archive_whole_or_not_at_all() {
    let daemon = daemon().await;
    let (root, project, snapshot) = captured(&daemon).await;
    let file = daemon.scratch("downloads").join("project.zip");
    let archive = file.to_string_lossy().into_owned();

    let run = daemon
        .run_cli_in(&project, KEEPSTER, &["export", "--archive", &archive])
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let size = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots/{snapshot}/stat"))
        .await;
    assert_eq!(
        std::fs::metadata(&file).expect("the archive").len(),
        size["archive_bytes"].as_u64().expect("a size")
    );
    let names: Vec<String> = std::fs::read_dir(file.parent().expect("its directory"))
        .expect("the directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["project.zip"], "nothing partial is left beside it");

    let missing = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &["export", "nowhere.txt", "--archive", &archive],
        )
        .await;
    assert_eq!(missing.exit_code, Some(4), "{}", missing.stderr);
    assert_eq!(
        std::fs::metadata(&file).expect("still there").len(),
        size["archive_bytes"].as_u64().expect("a size"),
        "a refusal leaves what was there"
    );
}

#[tokio::test]
async fn a_file_landing_in_another_root_beneath_the_destination_is_refused() {
    let daemon = daemon().await;
    let (root, _, snapshot) = captured(&daemon).await;
    // A Root at `docs` beneath the destination: `docs/guide.md` would land in it.
    let dest = daemon.scratch("exports");
    let other = dest.join("docs");
    std::fs::create_dir_all(&other).expect("another Root");
    daemon
        .post_json("/v1/roots", &json!({ "path": other }))
        .await;

    let refused = export(
        &daemon,
        &root,
        json!({ "from": snapshot, "dest": dest, "confirm": "all" }),
    )
    .await;

    assert_eq!(refused.status.as_u16(), 400);
    let problem = body(&refused);
    let docs = problem["paths"]
        .as_array()
        .expect("paths")
        .iter()
        .find(|path| path["path"] == "docs/guide.md")
        .expect("the file landing in the other Root");
    assert_eq!(docs["outcome"], "blocked", "{problem}");
    assert_eq!(docs["issues"][0]["reason"], "other_root", "{problem}");

    let skipping = finished(
        &daemon,
        &export(
            &daemon,
            &root,
            json!({ "from": snapshot, "dest": dest, "skip_unresolvable": true }),
        )
        .await,
    )
    .await;
    assert!(
        !other.join("guide.md").exists(),
        "never written into a Root that is not the destination's: {skipping}"
    );
    assert!(dest.join("notes.txt").exists());
}

#[tokio::test]
async fn a_destination_climbing_back_into_a_history_store_is_still_refused() {
    let daemon = daemon().await;
    let (root, _, snapshot) = captured(&daemon).await;
    let climbing = daemon
        .scratch("exports")
        .join("new")
        .join("..")
        .join("..")
        .join("store")
        .join(&root);

    let refused = export(
        &daemon,
        &root,
        json!({ "from": snapshot, "dest": climbing }),
    )
    .await;

    assert_eq!(refused.status.as_u16(), 400);
    assert_eq!(body(&refused)["code"], "dest_in_store");
}
