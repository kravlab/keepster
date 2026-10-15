//! Names that are not UTF-8 through `/v1` (ticket 74): held whole, reached by the bytes a `path=`
//! parameter percent-encodes on every endpoint, and named on the wire by their lossy text plus a
//! `_bytes` companion in base64url wherever a path appears — a request's companion winning over its
//! text. Linux only: macOS refuses such a name and Windows spells names in UTF-16.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use keepster_cli::Response;
use keepster_testkit::{Harness, MANUAL};
use serde_json::{Value, json};

/// The name every test holds: `\xff.txt`, which no UTF-8 spells.
const ODD: &[u8] = b"\xff.txt";
/// [`ODD`] as the wire's text carries it.
const ODD_TEXT: &str = "\u{fffd}.txt";
/// [`ODD`] as its `_bytes` companion carries it.
const ODD_BYTES: &str = "_y50eHQ";
/// [`ODD`] as a `path=` parameter carries it.
const ODD_QUERY: &str = "%FF.txt";

/// A daemon that captures only when asked, withholds any file over 64 bytes, and follows a link whose
/// name ends `.link`.
async fn daemon() -> Harness {
    let config = format!(
        "{}\n[files]\nmax_size = \"64 B\"\nfollow_symlinks = [\"*.link\"]\n",
        String::from_utf8_lossy(MANUAL)
    );
    Harness::start_with_global_config(config.as_bytes()).await
}

/// `name`, which need not be UTF-8, in `directory`.
fn at(directory: &Path, name: &[u8]) -> PathBuf {
    directory.join(OsStr::from_bytes(name))
}

/// A registered Root holding `notes.txt` and `\xff.txt`, inside a directory `\xfe` holding `inner.txt`
/// too; its id and its directory.
async fn registered(daemon: &Harness) -> (String, PathBuf) {
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.txt"), b"plain\n").expect("a file");
    std::fs::write(at(&project, ODD), b"odd\n").expect("a file with a name that is not UTF-8");
    std::fs::create_dir_all(at(&project, b"\xfe")).expect("a directory with such a name");
    std::fs::write(at(&project, b"\xfe/inner.txt"), b"inner\n").expect("a file inside it");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();

    (root, project)
}

async fn take(daemon: &Harness, root: &str) -> Value {
    daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await
}

fn body(response: &Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

fn header(response: &Response, name: &str) -> String {
    response
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// The job an accepted request started, once it has ended.
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

/// Every line of an NDJSON answer.
fn records(response: &Response) -> Vec<Value> {
    assert_eq!(
        response.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&response.body)
    );
    String::from_utf8_lossy(&response.body)
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is JSON"))
        .collect()
}

#[tokio::test]
async fn a_snapshot_holds_the_name_whole_and_lists_it_by_its_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    std::fs::write(at(&project, b"\xfe.bin"), [b'z'; 100]).expect("an over-cap file");

    let snapshot = take(&daemon, &root).await;

    assert_eq!(snapshot["incomplete"], false, "{snapshot}");
    assert_eq!(
        snapshot["withheld"],
        json!([{ "path": "\u{fffd}.bin", "path_bytes": "_i5iaW4", "reason": "too_large" }]),
        "{snapshot}"
    );
    let id = snapshot["id"].as_str().expect("an id");
    let listed = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots/{id}/entries?depth=infinity"
        ))
        .await;
    let named: Vec<(&str, Option<&str>)> = listed["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["path"].as_str().expect("a path"),
                entry["path_bytes"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        named,
        [
            ("notes.txt", None),
            ("\u{fffd}", Some("_g")),
            ("\u{fffd}.bin", Some("_i5iaW4")),
            ("\u{fffd}/inner.txt", Some("_i9pbm5lci50eHQ")),
            (ODD_TEXT, Some(ODD_BYTES)),
        ],
        "in byte order, each with its bytes where the text cannot carry them"
    );
}

#[tokio::test]
async fn an_omitted_name_is_named_by_its_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    std::os::unix::fs::symlink(project.join("gone"), at(&project, b"\xfe.link"))
        .expect("a followed link to nothing");

    let snapshot = take(&daemon, &root).await;

    assert_eq!(snapshot["incomplete"], true, "{snapshot}");
    assert_eq!(snapshot["omitted"], json!(["\u{fffd}.link"]), "{snapshot}");
    assert_eq!(snapshot["omitted_bytes"], json!(["_i5saW5r"]), "{snapshot}");
}

#[tokio::test]
async fn every_path_parameter_takes_the_name_s_exact_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");

    let read = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{id}/content?path={ODD_QUERY}&download=1"
        ))
        .await;
    assert_eq!(
        read.status.as_u16(),
        200,
        "{}",
        String::from_utf8_lossy(&read.body)
    );
    assert_eq!(read.body, b"odd\n");
    assert_eq!(
        header(&read, "content-disposition"),
        "attachment; filename=\"_.txt\"; filename*=UTF-8''%EF%BF%BD.txt",
        "named by its lossy text, which a header can carry"
    );

    let inside = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots/{id}/entries?path=%FE"))
        .await;
    assert_eq!(
        inside["entries"][0]["path_bytes"], "_i9pbm5lci50eHQ",
        "{inside}"
    );

    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots?path={ODD_QUERY}"))
        .await;
    assert_eq!(timeline["snapshots"][0]["change"], "added", "{timeline}");

    let explained = daemon
        .get_json(&format!("/v1/roots/{root}/explain?path={ODD_QUERY}"))
        .await;
    assert_eq!(
        (
            &explained["path"],
            &explained["path_bytes"],
            &explained["captured"]
        ),
        (&json!(ODD_TEXT), &json!(ODD_BYTES), &json!(true)),
        "{explained}"
    );

    let size = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots/{id}/stat?path={ODD_QUERY}"
        ))
        .await;
    assert_eq!(
        (size["files"].as_u64(), size["bytes"].as_u64()),
        (Some(1), Some(4)),
        "{size}"
    );

    std::fs::write(at(&project, ODD), b"odder\n").expect("an edit");
    let changed = daemon
        .get_json(&format!("/v1/roots/{root}/diff?from={id}&to=working"))
        .await;
    assert_eq!(
        (
            &changed["files"][0]["path"],
            &changed["files"][0]["path_bytes"]
        ),
        (&json!(ODD_TEXT), &json!(ODD_BYTES)),
        "{changed}"
    );
    let one = daemon
        .get_json(&format!(
            "/v1/roots/{root}/diff?from={id}&to=working&path={ODD_QUERY}"
        ))
        .await;
    assert_eq!(one["files"][0]["path_bytes"], ODD_BYTES, "{one}");
    assert!(
        one["files"][0]["hunks"]
            .as_array()
            .is_some_and(|hunks| !hunks.is_empty()),
        "{one}"
    );

    let found = records(
        &daemon
            .get(&format!("/v1/search/content?q=inner&roots={root}&path=%FE"))
            .await,
    );
    let matched: Vec<&Value> = found
        .iter()
        .filter(|record| record["type"] == "match")
        .collect();
    assert_eq!(matched.len(), 1, "{found:?}");
    assert_eq!(matched[0]["path_bytes"], "_i9pbm5lci50eHQ");
}

#[tokio::test]
async fn a_path_that_is_not_one_is_refused_with_its_bytes_named() {
    let daemon = daemon().await;
    let (root, _) = registered(&daemon).await;
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");

    let refused = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{id}/content?path=%FF%00"
        ))
        .await;

    assert_eq!(refused.status.as_u16(), 400);
    let problem = body(&refused);
    assert_eq!(problem["code"], "path_invalid", "{problem}");
    assert_eq!(problem["details"]["path"], "\u{fffd}\u{0}", "{problem}");
    assert_eq!(problem["details"]["path_bytes"], "_wA", "{problem}");
}

#[tokio::test]
async fn a_path_parameter_with_a_broken_escape_is_refused() {
    let daemon = daemon().await;
    let (root, _) = registered(&daemon).await;
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");

    let refused = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{id}/content?path=%G1.txt"
        ))
        .await;

    assert_eq!(refused.status.as_u16(), 400);
    assert_eq!(body(&refused)["code"], "path_invalid");
}

#[tokio::test]
async fn the_archive_names_the_file_by_its_bytes_and_claims_utf8_only_where_it_is() {
    let daemon = daemon().await;
    let (root, _) = registered(&daemon).await;
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");

    let archive = daemon
        .get(&format!("/v1/roots/{root}/snapshots/{id}/archive"))
        .await;
    assert_eq!(archive.status.as_u16(), 200);

    // Every local file header: its general-purpose flags, and its name.
    let mut names = Vec::new();
    let mut at = 0;
    while let Some(found) = archive.body[at..]
        .windows(4)
        .position(|window| window == b"PK\x03\x04")
    {
        let header = &archive.body[at + found..];
        let flags = u16::from_le_bytes([header[6], header[7]]);
        let length = usize::from(u16::from_le_bytes([header[26], header[27]]));
        names.push((header[30..30 + length].to_vec(), flags & 0x0800 != 0));
        at += found + 30 + length;
    }
    assert!(
        names.contains(&(ODD.to_vec(), false)),
        "the exact bytes, not declared UTF-8: {names:?}"
    );
    assert!(names.contains(&(b"notes.txt".to_vec(), true)), "{names:?}");
}

#[tokio::test]
async fn restore_writes_the_file_back_under_its_exact_name() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");
    std::fs::remove_file(at(&project, ODD)).expect("removed");

    let restored = daemon
        .post(
            &format!("/v1/roots/{root}/restore"),
            &json!({ "from": id, "paths": [ODD_TEXT], "paths_bytes": [ODD_BYTES] }),
        )
        .await;

    assert_eq!(restored.status.as_u16(), 200, "{}", body(&restored));
    let answer = body(&restored);
    assert_eq!(
        (
            &answer["paths"][0]["path_bytes"],
            &answer["paths"][0]["outcome"]
        ),
        (&json!(ODD_BYTES), &json!("created")),
        "{answer}"
    );
    assert_eq!(
        std::fs::read(at(&project, ODD)).expect("written back"),
        b"odd\n"
    );
}

#[tokio::test]
async fn a_resolution_keyed_by_bytes_answers_for_its_path() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    let script = at(&project, b"\xff.sh");
    std::fs::write(&script, b"run\n").expect("a file");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o600)).expect("its mode");
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).expect("loosened");
    let url = format!("/v1/roots/{root}/restore");

    let asked = daemon
        .post(
            &url,
            &json!({ "from": id, "paths": ["\u{fffd}.sh"], "paths_bytes": ["_y5zaA"] }),
        )
        .await;
    let problem = body(&asked);
    assert_eq!(problem["code"], "confirmation_required", "{problem}");
    assert_eq!(problem["paths"][0]["path_bytes"], "_y5zaA", "{problem}");
    assert_eq!(
        problem["paths"][0]["issues"][0]["reason"], "modes_differ",
        "{problem}"
    );

    let answered = daemon
        .post(
            &url,
            &json!({
                "from": id,
                "paths": ["\u{fffd}.sh"],
                "paths_bytes": ["_y5zaA"],
                "resolutions_bytes": { "_y5zaA": { "modes": "snapshot" } },
            }),
        )
        .await;
    assert_eq!(answered.status.as_u16(), 200, "{}", body(&answered));
    assert_eq!(
        std::fs::metadata(&script)
            .expect("there")
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
}

#[tokio::test]
async fn a_hardlinked_file_s_other_name_is_named_by_its_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");
    std::fs::write(project.join("notes.txt"), b"since\n").expect("an edit");
    std::fs::remove_file(at(&project, ODD)).expect("removed");
    std::fs::hard_link(project.join("notes.txt"), at(&project, ODD)).expect("a twin");

    let refused = body(
        &daemon
            .post(
                &format!("/v1/roots/{root}/restore"),
                &json!({ "from": id, "paths": ["notes.txt"] }),
            )
            .await,
    );

    let issue = &refused["paths"][0]["issues"][0];
    assert_eq!(issue["reason"], "hardlinked", "{refused}");
    assert_eq!(issue["paths"], json!([ODD_TEXT]), "{refused}");
    assert_eq!(issue["paths_bytes"], json!([ODD_BYTES]), "{refused}");
}

#[tokio::test]
async fn export_writes_the_file_under_its_exact_name() {
    let daemon = daemon().await;
    let (root, _) = registered(&daemon).await;
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");
    let dest = daemon.scratch("exported");

    let accepted = daemon
        .post(
            &format!("/v1/roots/{root}/export"),
            &json!({ "from": id, "paths": [ODD_TEXT], "paths_bytes": [ODD_BYTES], "dest": dest }),
        )
        .await;
    let job = finished(&daemon, &accepted).await;

    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(job["result"]["paths"][0]["path_bytes"], ODD_BYTES, "{job}");
    assert_eq!(std::fs::read(at(&dest, ODD)).expect("exported"), b"odd\n");
}

#[tokio::test]
async fn forget_names_what_it_removed_by_its_bytes() {
    let daemon = daemon().await;
    let (root, _) = registered(&daemon).await;
    take(&daemon, &root).await;

    let accepted = daemon
        .post(
            &format!("/v1/roots/{root}/forget"),
            &json!({ "glob": "?.txt", "confirm": true, "on_exclude": "keep-watching" }),
        )
        .await;
    let job = finished(&daemon, &accepted).await;

    let result = &job["result"];
    assert_eq!(result["removed_paths"], json!([ODD_TEXT]), "{job}");
    assert_eq!(result["removed_paths_bytes"], json!([ODD_BYTES]), "{job}");
    assert_eq!(result["still_on_disk_bytes"], json!([ODD_BYTES]), "{job}");
}

#[tokio::test]
async fn a_path_search_and_a_preview_name_it_by_its_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    take(&daemon, &root).await;

    let found = daemon.get_json("/v1/search/paths?q=.txt").await;
    let hit = found["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .find(|hit| hit["path"] == ODD_TEXT)
        .unwrap_or_else(|| panic!("found: {found}"));
    assert_eq!(hit["path_bytes"], ODD_BYTES);

    let previewed = records(
        &daemon
            .get(&format!(
                "/v1/preview?path={}&depth=infinity",
                keepster_api::encode_path_query(project.as_os_str().as_bytes())
            ))
            .await,
    );
    let entry = previewed
        .iter()
        .find(|record| record["type"] == "entry" && record["path_bytes"] == ODD_BYTES)
        .unwrap_or_else(|| panic!("previewed: {previewed:?}"));
    assert_eq!(entry["class"], "captured");
}

/// The CLI run from inside `directory` with `args`, which need not be UTF-8.
async fn cli(daemon: &Harness, directory: &Path, args: &[&[u8]]) -> keepster_testkit::CliRun {
    let args: Vec<&OsStr> = args.iter().map(|arg| OsStr::from_bytes(arg)).collect();
    daemon
        .run_cli_in_os(directory, env!("CARGO_BIN_EXE_keepster"), &args)
        .await
}

#[tokio::test]
async fn the_cli_reads_restores_and_prints_the_name_by_its_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    take(&daemon, &root).await;

    let read = cli(&daemon, &project, &[b"cat", ODD]).await;
    assert_eq!(read.exit_code, Some(0), "{}", read.stderr);
    assert_eq!(read.stdout, b"odd\n");

    std::fs::remove_file(at(&project, ODD)).expect("removed");
    let restored = cli(&daemon, &project, &[b"restore", ODD]).await;
    assert_eq!(restored.exit_code, Some(0), "{}", restored.stderr);
    assert_eq!(
        std::fs::read(at(&project, ODD)).expect("written back"),
        b"odd\n"
    );
    assert!(
        restored.stdout.starts_with(b"created  \xff.txt\n"),
        "printed as the name is, as `ls` prints it: {:?}",
        String::from_utf8_lossy(&restored.stdout)
    );
}

#[tokio::test]
async fn a_cli_answer_names_its_path_by_its_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    let script = at(&project, b"\xff.sh");
    std::fs::write(&script, b"run\n").expect("a file");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o600)).expect("its mode");
    take(&daemon, &root).await;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).expect("loosened");

    let asked = cli(&daemon, &project, &[b"restore", b"\xff.sh", b"--no-input"]).await;
    assert_eq!(asked.exit_code, Some(5), "{}", asked.stderr);

    let answered = cli(
        &daemon,
        &project,
        &[b"restore", b"\xff.sh", b"--modes", b"\xff.sh=snapshot"],
    )
    .await;
    assert_eq!(answered.exit_code, Some(0), "{}", answered.stderr);
    assert_eq!(
        std::fs::metadata(&script)
            .expect("there")
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
}

#[tokio::test]
async fn the_cli_lists_a_change_by_the_name_s_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    let snapshot = take(&daemon, &root).await;
    let id = snapshot["id"].as_str().expect("an id");
    std::fs::write(at(&project, ODD), b"odder\n").expect("an edit");

    let listed = cli(&daemon, &project, &[b"diff", b"--from", id.as_bytes()]).await;

    assert_eq!(listed.exit_code, Some(0), "{}", listed.stderr);
    assert!(
        listed
            .stdout
            .windows(9)
            .any(|window| window == b"M  \xff.txt\n"),
        "{:?}",
        String::from_utf8_lossy(&listed.stdout)
    );
}

#[tokio::test]
async fn the_cli_explains_and_finds_the_name_by_its_bytes() {
    let daemon = daemon().await;
    let (root, project) = registered(&daemon).await;
    take(&daemon, &root).await;

    let explained = cli(&daemon, &project, &[b"explain", ODD]).await;
    assert_eq!(explained.exit_code, Some(0), "{}", explained.stderr);
    assert!(
        explained.stdout.starts_with(b"\xff.txt: captured\n"),
        "{:?}",
        String::from_utf8_lossy(&explained.stdout)
    );

    let found = cli(&daemon, &project, &[b"find", b".txt"]).await;
    assert_eq!(found.exit_code, Some(0), "{}", found.stderr);
    let expected = [at(&project, ODD).as_os_str().as_bytes(), b"  1 Snapshot"].concat();
    assert!(
        found
            .stdout
            .windows(expected.len())
            .any(|window| window == expected),
        "{:?}",
        String::from_utf8_lossy(&found.stdout)
    );
}
