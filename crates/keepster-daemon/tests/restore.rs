//! Restore at the `/v1` seam (ticket 47), against a real store and a real Working Tree: the whole request
//! planned before a byte is written, every structural change confirmed, and one Safety Snapshot that
//! undoes it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use keepster_cli::Response;
use keepster_testkit::Harness;
use serde_json::{Value, json};

async fn registered(daemon: &Harness) -> (String, PathBuf) {
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await;

    (root["id"].as_str().expect("an id").to_owned(), project)
}

fn write(project: &Path, path: &str, content: &[u8]) {
    let file = project.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, content).expect("a file");
}

fn read(project: &Path, path: &str) -> Vec<u8> {
    std::fs::read(project.join(path)).expect("the file")
}

async fn take(daemon: &Harness, root: &str) -> String {
    let taken = daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await;

    taken["id"].as_str().expect("an id").to_owned()
}

async fn restore(daemon: &Harness, root: &str, body: Value) -> Response {
    daemon
        .post(&format!("/v1/roots/{root}/restore"), &body)
        .await
}

fn body(response: &Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

/// Each file's outcome, as `(path, outcome)`.
fn outcomes(answer: &Value) -> Vec<(String, String)> {
    answer["paths"]
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

async fn timeline_length(daemon: &Harness, root: &str) -> usize {
    daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await["snapshots"]
        .as_array()
        .map_or(0, Vec::len)
}

async fn content_at(daemon: &Harness, root: &str, snapshot: &str, path: &str) -> Vec<u8> {
    daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/content?path={path}"
        ))
        .await
        .body
}

#[tokio::test]
async fn a_restored_file_is_written_back_and_its_safety_snapshot_undoes_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"what was saved");
    let saved = take(&daemon, &root).await;
    write(&project, "notes.txt", b"what was written since");

    let answer = restore(
        &daemon,
        &root,
        json!({ "from": saved, "paths": ["notes.txt"] }),
    )
    .await;

    assert_eq!(
        answer.status,
        200,
        "{}",
        String::from_utf8_lossy(&answer.body)
    );
    let answer = body(&answer);
    assert_eq!(outcomes(&answer), pairs(&[("notes.txt", "restored")]));
    assert_eq!(read(&project, "notes.txt"), b"what was saved");
    let safety = answer["safety_snapshot"]
        .as_str()
        .expect("a Safety Snapshot");
    let shown = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(shown["snapshots"][0]["id"], safety);
    assert_eq!(shown["snapshots"][0]["trigger"], "restore");
    assert_eq!(
        content_at(&daemon, &root, safety, "notes.txt").await,
        b"what was written since",
        "the Safety Snapshot holds what the restore replaced"
    );

    let undone = restore(
        &daemon,
        &root,
        json!({ "from": safety, "paths": ["notes.txt"] }),
    )
    .await;
    assert_eq!(undone.status, 200);
    assert_eq!(read(&project, "notes.txt"), b"what was written since");
}

#[tokio::test]
async fn a_directory_restores_everything_beneath_it_and_says_what_happened_to_each() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "src/edited.rs", b"fn saved() {}");
    write(&project, "src/deleted.rs", b"fn gone() {}");
    write(&project, "src/kept.rs", b"fn kept() {}");
    let saved = take(&daemon, &root).await;
    write(&project, "src/edited.rs", b"fn since() {}");
    std::fs::remove_file(project.join("src/deleted.rs")).expect("deleted");
    write(&project, "src/added.rs", b"fn new() {}");

    let answer = body(&restore(&daemon, &root, json!({ "from": saved, "paths": ["src"] })).await);

    assert_eq!(
        outcomes(&answer),
        pairs(&[
            ("src/deleted.rs", "created"),
            ("src/edited.rs", "restored"),
            ("src/kept.rs", "unchanged"),
        ])
    );
    assert_eq!(
        read(&project, "src/added.rs"),
        b"fn new() {}",
        "a restore never deletes"
    );
}

#[tokio::test]
async fn a_working_tree_identical_to_the_latest_snapshot_takes_no_second_one() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"one");
    let first = take(&daemon, &root).await;
    write(&project, "notes.txt", b"two");
    let latest = take(&daemon, &root).await;

    let answer = body(&restore(&daemon, &root, json!({ "from": first })).await);

    assert_eq!(answer["safety_snapshot"], latest.as_str());
    assert_eq!(timeline_length(&daemon, &root).await, 2);
    assert_eq!(read(&project, "notes.txt"), b"one");
}

#[tokio::test]
async fn a_dry_run_plans_everything_and_writes_nothing() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved");
    write(&project, "deep/file.txt", b"saved");
    let saved = take(&daemon, &root).await;
    write(&project, "notes.txt", b"since");
    std::fs::remove_dir_all(project.join("deep")).expect("removed");

    let answer = restore(&daemon, &root, json!({ "from": saved, "dry_run": true })).await;

    assert_eq!(answer.status, 200);
    let answer = body(&answer);
    assert_eq!(answer["dry_run"], true);
    assert!(answer.get("safety_snapshot").is_none());
    assert_eq!(
        outcomes(&answer),
        pairs(&[("deep/file.txt", "blocked"), ("notes.txt", "restored")])
    );
    assert_eq!(
        answer["paths"][0]["issues"],
        json!([{ "reason": "parent_missing", "confirm": "create_parents" }])
    );
    assert_eq!(read(&project, "notes.txt"), b"since");
    assert_eq!(
        timeline_length(&daemon, &root).await,
        1,
        "no Safety Snapshot"
    );
}

#[tokio::test]
async fn a_path_that_needs_a_confirmation_stops_the_whole_request_until_it_is_given() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved");
    write(&project, "deep/file.txt", b"saved");
    let saved = take(&daemon, &root).await;
    write(&project, "notes.txt", b"since");
    std::fs::remove_dir_all(project.join("deep")).expect("removed");

    let refused = restore(&daemon, &root, json!({ "from": saved })).await;

    assert_eq!(refused.status, 400);
    let problem = body(&refused);
    assert_eq!(problem["code"], "confirmation_required");
    assert_eq!(
        outcomes(&problem),
        pairs(&[("deep/file.txt", "blocked"), ("notes.txt", "restored")])
    );
    assert_eq!(read(&project, "notes.txt"), b"since", "nothing was written");
    assert_eq!(
        timeline_length(&daemon, &root).await,
        1,
        "and no Safety Snapshot taken"
    );

    let confirmed = restore(
        &daemon,
        &root,
        json!({ "from": saved, "confirm": ["create_parents"] }),
    )
    .await;
    assert_eq!(confirmed.status, 200);
    assert_eq!(read(&project, "deep/file.txt"), b"saved");
    assert_eq!(read(&project, "notes.txt"), b"saved");
}

#[cfg(unix)]
#[tokio::test]
async fn a_hardlinked_file_is_named_with_its_other_links_and_breaking_them_leaves_those_alone() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved");
    let saved = take(&daemon, &root).await;
    write(&project, "notes.txt", b"since");
    std::fs::hard_link(project.join("notes.txt"), project.join("twin.txt")).expect("a twin");
    let outside = daemon.scratch("elsewhere").join("outside.txt");
    std::fs::hard_link(project.join("notes.txt"), &outside).expect("a link outside");

    let refused = body(
        &restore(
            &daemon,
            &root,
            json!({ "from": saved, "paths": ["notes.txt"] }),
        )
        .await,
    );

    assert_eq!(
        refused["paths"][0]["issues"],
        json!([{
            "reason": "hardlinked",
            "confirm": "break_hardlinks",
            "paths": ["twin.txt"],
            "links_outside": 1,
        }])
    );

    let confirmed = restore(
        &daemon,
        &root,
        json!({ "from": saved, "paths": ["notes.txt"], "confirm": ["break_hardlinks"] }),
    )
    .await;
    assert_eq!(confirmed.status, 200);
    assert_eq!(read(&project, "notes.txt"), b"saved");
    assert_eq!(
        read(&project, "twin.txt"),
        b"since",
        "the other names keep what they had"
    );
    assert_eq!(std::fs::read(&outside).expect("outside"), b"since");
}

#[tokio::test]
async fn overwriting_what_no_snapshot_can_hold_is_confirmed_and_the_blanket_force_does_it() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "debug.log", b"saved before logs were excluded");
    let saved = take(&daemon, &root).await;
    daemon.write_global_config(
        b"[triggers]\nactive = []\n\n[reconcile]\nat_start = false\n\n[files]\nexclude = [\"*.log\"]\n",
    );
    write(
        &project,
        "debug.log",
        b"excluded now, so no Snapshot keeps this",
    );

    // The new rules are read within a second.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let refused = loop {
        let answer = body(
            &restore(
                &daemon,
                &root,
                json!({ "from": saved, "paths": ["debug.log"], "dry_run": true }),
            )
            .await,
        );
        if answer["paths"][0]["outcome"] == "blocked" {
            break answer;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the exclusion was never read"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(
        refused["paths"][0]["issues"],
        json!([{
            "reason": "overwrite_unrecoverable",
            "confirm": "overwrite_unrecoverable",
            "why": "excluded",
        }])
    );

    let forced = restore(
        &daemon,
        &root,
        json!({ "from": saved, "paths": ["debug.log"], "confirm": "all" }),
    )
    .await;
    assert_eq!(
        forced.status,
        200,
        "{}",
        String::from_utf8_lossy(&forced.body)
    );
    assert_eq!(
        read(&project, "debug.log"),
        b"saved before logs were excluded"
    );
}

#[tokio::test]
async fn a_file_in_the_way_of_a_directory_is_replaced_only_when_confirmed() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "docs/guide.md", b"saved");
    let saved = take(&daemon, &root).await;
    std::fs::remove_dir_all(project.join("docs")).expect("removed");
    write(&project, "docs", b"a file where a directory was");

    let refused = body(&restore(&daemon, &root, json!({ "from": saved })).await);
    assert_eq!(
        refused["paths"][0]["issues"],
        json!([{ "reason": "type_change", "confirm": "replace_type", "at": "docs" }])
    );

    let confirmed = restore(
        &daemon,
        &root,
        json!({ "from": saved, "confirm": ["replace_type"] }),
    )
    .await;
    assert_eq!(confirmed.status, 200);
    assert_eq!(read(&project, "docs/guide.md"), b"saved");
    let safety = body(&confirmed)["safety_snapshot"]
        .as_str()
        .expect("an id")
        .to_owned();
    assert_eq!(
        content_at(&daemon, &root, &safety, "docs").await,
        b"a file where a directory was",
        "the replaced file is in the Safety Snapshot"
    );
}

#[tokio::test]
async fn a_directory_where_the_file_goes_is_refused_and_skipping_lets_the_rest_through() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "report", b"saved as a file");
    write(&project, "notes.txt", b"saved");
    let saved = take(&daemon, &root).await;
    std::fs::remove_file(project.join("report")).expect("removed");
    write(&project, "report/inside.txt", b"a directory now");
    write(&project, "notes.txt", b"since");

    let refused = restore(&daemon, &root, json!({ "from": saved, "confirm": "all" })).await;
    assert_eq!(
        refused.status, 400,
        "the blanket force does not replace a directory"
    );
    assert_eq!(
        body(&refused)["paths"][1]["issues"],
        json!([{ "reason": "type_conflict" }])
    );

    let skipped = body(
        &restore(
            &daemon,
            &root,
            json!({ "from": saved, "skip_unresolvable": true }),
        )
        .await,
    );
    assert_eq!(
        outcomes(&skipped),
        pairs(&[("notes.txt", "restored"), ("report", "skipped")])
    );
    assert_eq!(read(&project, "report/inside.txt"), b"a directory now");
}

#[cfg(unix)]
#[tokio::test]
async fn a_write_through_a_link_out_of_the_root_is_refused() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "shared/file.txt", b"saved");
    let saved = take(&daemon, &root).await;
    std::fs::remove_dir_all(project.join("shared")).expect("removed");
    let elsewhere = daemon.scratch("elsewhere");
    std::os::unix::fs::symlink(&elsewhere, project.join("shared")).expect("a link");

    let refused = body(&restore(&daemon, &root, json!({ "from": saved, "confirm": "all" })).await);

    assert_eq!(
        refused["paths"][0]["issues"],
        json!([{ "reason": "link_check_failed", "at": "shared" }])
    );
    assert!(!elsewhere.join("file.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_restored_file_takes_the_snapshots_permission_bits_when_asked_to() {
    use std::os::unix::fs::PermissionsExt;

    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    let chmod = |path: &str, mode: u32| {
        std::fs::set_permissions(project.join(path), std::fs::Permissions::from_mode(mode))
            .expect("its mode");
    };
    write(&project, "run.sh", b"echo saved");
    chmod("run.sh", 0o750);
    write(&project, "plain.txt", b"saved");
    chmod("plain.txt", 0o640);
    let saved = take(&daemon, &root).await;
    write(&project, "run.sh", b"echo since");
    chmod("run.sh", 0o600);
    std::fs::remove_file(project.join("plain.txt")).expect("deleted");

    let answer = restore(
        &daemon,
        &root,
        json!({ "from": saved, "modes": "snapshot" }),
    )
    .await;
    assert_eq!(answer.status, 200, "{}", body(&answer));

    let mode = |path: &str| {
        std::fs::metadata(project.join(path))
            .expect("the file")
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(mode("run.sh"), 0o750, "the Snapshot's bits, all of them");
    assert_eq!(
        mode("plain.txt"),
        0o640,
        "a new file takes the Snapshot's bits, not the umask's"
    );
}

#[tokio::test]
async fn a_restore_is_announced_on_the_event_stream() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved");
    let saved = take(&daemon, &root).await;
    write(&project, "notes.txt", b"since");
    let mut stream = daemon.events(None).await;

    let answer = body(&restore(&daemon, &root, json!({ "from": saved })).await);

    let completed = loop {
        let event = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("an event")
            .expect("the stream reads")
            .expect("open");
        if event.event == "restore.completed" {
            break serde_json::from_str::<Value>(&event.data).expect("JSON");
        }
    };
    assert_eq!(
        completed,
        json!({
            "event": "restore.completed",
            "root": root,
            "safety_snapshot": answer["safety_snapshot"],
            "restored": 1,
            "created": 0,
            "failed": 0,
        })
    );
}

#[tokio::test]
async fn a_request_that_cannot_be_planned_is_refused_with_a_reason() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"saved");
    let saved = take(&daemon, &root).await;

    for (request, status, code) in [
        (
            json!({ "from": "01K6DX3QR7M2V8C9WZ4PN5YQ1A" }),
            404,
            "snapshot_not_found",
        ),
        (
            json!({ "from": saved, "paths": ["nowhere.txt"] }),
            404,
            "path_not_found",
        ),
        (
            json!({ "from": saved, "paths": ["../escape"] }),
            400,
            "path_invalid",
        ),
        (
            json!({ "from": saved, "resolutions": { "notes.txt": { "write": true, "rename_to": "x" } } }),
            400,
            "resolution_invalid",
        ),
        (
            json!({ "from": saved, "resolutions": { "notes.txt": { "rename_to": "a/b" } } }),
            400,
            "resolution_invalid",
        ),
    ] {
        let answer = restore(&daemon, &root, request.clone()).await;
        assert_eq!(answer.status, status, "{request}");
        assert_eq!(body(&answer)["code"], code, "{request}");
    }
}

#[tokio::test]
async fn files_sharing_a_missing_directory_are_all_restored_by_one_request() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "deep/a.txt", b"a");
    write(&project, "deep/b.txt", b"b");
    write(&project, "docs/one.md", b"one");
    write(&project, "docs/two.md", b"two");
    let saved = take(&daemon, &root).await;
    std::fs::remove_dir_all(project.join("deep")).expect("removed");
    std::fs::remove_dir_all(project.join("docs")).expect("removed");
    write(&project, "docs", b"a file where a directory was");

    let answer = restore(
        &daemon,
        &root,
        json!({ "from": saved, "confirm": ["create_parents", "replace_type"] }),
    )
    .await;

    assert_eq!(
        answer.status,
        200,
        "{}",
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        outcomes(&body(&answer)),
        pairs(&[
            ("deep/a.txt", "created"),
            ("deep/b.txt", "created"),
            ("docs/one.md", "created"),
            ("docs/two.md", "created"),
        ]),
        "writing one file of a directory does not make the next one look changed"
    );
    assert_eq!(read(&project, "docs/two.md"), b"two");
}

#[cfg(unix)]
#[tokio::test]
async fn both_names_of_a_hardlinked_file_restored_together_are_both_written() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "a.txt", b"saved a");
    write(&project, "b.txt", b"saved b");
    let saved = take(&daemon, &root).await;
    write(&project, "a.txt", b"since");
    std::fs::remove_file(project.join("b.txt")).expect("removed");
    std::fs::hard_link(project.join("a.txt"), project.join("b.txt")).expect("a twin");

    let answer = restore(
        &daemon,
        &root,
        json!({ "from": saved, "confirm": ["break_hardlinks"] }),
    )
    .await;

    assert_eq!(
        outcomes(&body(&answer)),
        pairs(&[("a.txt", "restored"), ("b.txt", "restored")])
    );
    assert_eq!(read(&project, "a.txt"), b"saved a");
    assert_eq!(read(&project, "b.txt"), b"saved b");
}

#[cfg(unix)]
#[tokio::test]
async fn a_special_file_in_the_way_is_one_no_snapshot_can_hold() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project) = registered(&daemon).await;
    write(&project, "pipe/file.txt", b"saved");
    let saved = take(&daemon, &root).await;
    std::fs::remove_dir_all(project.join("pipe")).expect("removed");
    let made = std::process::Command::new("mkfifo")
        .arg(project.join("pipe"))
        .status()
        .expect("mkfifo runs");
    assert!(made.success());

    let refused = body(
        &restore(
            &daemon,
            &root,
            json!({ "from": saved, "confirm": ["replace_type"] }),
        )
        .await,
    );

    assert_eq!(
        refused["paths"][0]["issues"],
        json!([{
            "reason": "overwrite_unrecoverable",
            "confirm": "overwrite_unrecoverable",
            "why": "unsupported",
        }]),
        "replacing it needs more than the type change agreed to"
    );
}
