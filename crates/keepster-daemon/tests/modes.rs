//! Permission bits at the `/v1` seam and from the command line (ticket 50): listed with each path,
//! compared by a diff, and — where a Restore finds them diverging — a question that the blanket force
//! never answers, answered by the request, by a path's own resolution or by `restore.modes`.
//!
//! Unix only: Windows keeps no permission bits.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use keepster_testkit::Harness;
use serde_json::{Value, json};

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

async fn registered(daemon: &Harness) -> (String, PathBuf) {
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await;

    (root["id"].as_str().expect("an id").to_owned(), project)
}

fn write(project: &Path, path: &str, mode: u32) {
    let file = project.join(path);
    std::fs::write(&file, b"content").expect("a file");
    chmod(&file, mode);
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("its mode");
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("the file")
        .permissions()
        .mode()
        & 0o7777
}

async fn take(daemon: &Harness, root: &str) -> String {
    let taken = daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await;

    taken["id"].as_str().expect("an id").to_owned()
}

fn body(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

/// A Root whose `key` was `0600` when the returned Snapshot was taken, and is `0644` now.
async fn loosened(daemon: &Harness) -> (String, PathBuf, String) {
    let (root, project) = registered(daemon).await;
    write(&project, "key", 0o600);
    let saved = take(daemon, &root).await;
    chmod(&project.join("key"), 0o644);

    (root, project, saved)
}

#[tokio::test]
async fn entries_and_a_diff_carry_all_twelve_bits() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project, saved) = loosened(&daemon).await;
    write(&project, "run.sh", 0o755);
    let now = take(&daemon, &root).await;

    let listed = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots/{now}/entries"))
        .await;
    let permissions: Vec<(&str, &Value)> = listed["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("a path"),
                &entry["permissions"],
            )
        })
        .collect();
    assert_eq!(
        permissions,
        [("key", &json!(0o644)), ("run.sh", &json!(0o755))],
        "{listed}"
    );

    let diff = daemon
        .get_json(&format!(
            "/v1/roots/{root}/diff?from={saved}&to={now}&path=key"
        ))
        .await;
    assert_eq!(
        (&diff["files"][0]["old_mode"], &diff["files"][0]["new_mode"]),
        (&json!(0o600), &json!(0o644)),
        "{diff}"
    );
}

#[tokio::test]
async fn diverging_modes_stop_a_restore_until_the_request_answers() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project, saved) = loosened(&daemon).await;
    let url = format!("/v1/roots/{root}/restore");

    let forced = daemon
        .post(&url, &json!({ "from": saved, "confirm": "all" }))
        .await;
    let problem = body(&forced);
    assert_eq!(forced.status, 400, "{problem}");
    assert_eq!(problem["code"], "confirmation_required");
    assert_eq!(
        problem["paths"][0]["issues"][0],
        json!({
            "reason": "modes_differ",
            "resolution": "modes",
            "snapshot_mode": 0o600,
            "current_mode": 0o644,
        }),
        "the blanket force does not answer it"
    );
    assert_eq!(mode(&project.join("key")), 0o644, "nothing was written");

    let kept = daemon
        .post(
            &url,
            &json!({ "from": saved, "resolutions": { "key": { "modes": "keep" } } }),
        )
        .await;
    assert_eq!(kept.status, 200, "{}", body(&kept));
    assert_eq!(body(&kept)["paths"][0]["outcome"], "unchanged");
    assert_eq!(mode(&project.join("key")), 0o644);

    let restored = daemon
        .post(&url, &json!({ "from": saved, "modes": "snapshot" }))
        .await;
    assert_eq!(restored.status, 200, "{}", body(&restored));
    assert_eq!(body(&restored)["paths"][0]["outcome"], "restored");
    assert_eq!(
        mode(&project.join("key")),
        0o600,
        "the Snapshot's bits, in place"
    );
}

#[tokio::test]
async fn the_configuration_can_answer_for_every_restore() {
    let daemon = Harness::start_with_global_config(
        b"[triggers]\nactive = []\n\n[reconcile]\nat_start = false\n\n[restore]\nmodes = \"snapshot\"\n",
    )
    .await;
    let (root, project, saved) = loosened(&daemon).await;

    let restored = daemon
        .post(
            &format!("/v1/roots/{root}/restore"),
            &json!({ "from": saved }),
        )
        .await;

    assert_eq!(restored.status, 200, "{}", body(&restored));
    assert_eq!(mode(&project.join("key")), 0o600);
}

#[tokio::test]
async fn the_command_line_names_the_flag_and_takes_the_answer() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (_, project, _) = loosened(&daemon).await;

    let refused = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &["restore", "key", "--force", "--no-input"],
        )
        .await;
    assert_eq!(refused.exit_code, Some(5), "{}", refused.stderr);
    assert!(
        refused
            .stderr
            .contains("0644 on disk, 0600 in the Snapshot")
            && refused.stderr.contains("--modes key=snapshot|keep"),
        "{}",
        refused.stderr
    );

    let restored = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &["restore", "key", "--modes", "snapshot"],
        )
        .await;
    assert_eq!(restored.exit_code, Some(0), "{}", restored.stderr);
    assert_eq!(mode(&project.join("key")), 0o600);
}
