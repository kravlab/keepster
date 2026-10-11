//! Include Rules, the secret denylist and the size cap at the `/v1` seam: what a Snapshot holds, what
//! `explain` says about any path, and what a configuration that cannot be read does to a capture.

use std::path::{Path, PathBuf};

use keepster_testkit::Harness;
use serde_json::Value;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

fn write(root: &Path, path: &str, content: &[u8]) {
    let file = root.join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("its directory");
    std::fs::write(file, content).expect("a file");
}

async fn registered(daemon: &Harness) -> (String, PathBuf) {
    let project = daemon.scratch("project");
    let root = daemon
        .post_json("/v1/roots", &serde_json::json!({ "path": project }))
        .await;

    (root["id"].as_str().expect("an id").to_owned(), project)
}

async fn take(daemon: &Harness, root: &str) -> Value {
    daemon
        .post_json(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await
}

/// Every file path the Snapshot holds.
async fn captured(daemon: &Harness, root: &str, snapshot: &Value) -> Vec<String> {
    let id = snapshot["id"].as_str().expect("an id");
    let listed = daemon
        .get_json(&format!(
            "/v1/roots/{root}/snapshots/{id}/entries?depth=infinity"
        ))
        .await;

    listed["entries"]
        .as_array()
        .expect("the entries")
        .iter()
        .filter(|entry| entry["kind"] == "file")
        .map(|entry| entry["path"].as_str().expect("a path").to_owned())
        .collect()
}

async fn explain(daemon: &Harness, root: &str, path: &str) -> Value {
    daemon
        .get_json(&format!(
            "/v1/roots/{root}/explain?path={}",
            keepster_api::encode_path_query(path)
        ))
        .await
}

#[tokio::test]
async fn a_snapshot_holds_nothing_the_rules_exclude() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(
        &project,
        ".keepster.toml",
        br#"
[files]
exclude = ["node_modules/", "*.log", "build/*"]
include = ["build/keep.txt", "node_modules/mylib/index.js"]
"#,
    );
    write(&project, "src/main.rs", b"fn main() {}");
    write(&project, "debug.log", b"noise");
    write(&project, "node_modules/mylib/index.js", b"vendored");
    write(&project, "build/out.o", b"object");
    write(&project, "build/keep.txt", b"kept");

    let snapshot = take(&daemon, &root).await;

    assert_eq!(
        captured(&daemon, &root, &snapshot).await,
        [".keepster.toml", "build/keep.txt", "src/main.rs"],
        "an include re-admits a file whose directory is entered, and nothing under a pruned one"
    );
}

#[tokio::test]
async fn excluding_a_path_later_removes_it_from_the_next_snapshot() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(&project, "notes.txt", b"kept");
    write(&project, "cache/blob.bin", b"cached");
    take(&daemon, &root).await;

    write(
        &project,
        ".keepster.toml",
        b"[files]\nexclude = [\"cache/\"]\n",
    );
    let after = take(&daemon, &root).await;

    assert_eq!(
        captured(&daemon, &root, &after).await,
        [".keepster.toml", "notes.txt"]
    );
}

#[tokio::test]
async fn secrets_are_refused_by_default_and_only_the_global_opt_in_admits_one() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(&project, ".env", b"TOKEN=secret");
    write(&project, ".env.example", b"TOKEN=");
    write(&project, "certs/server.pem", b"-----BEGIN");
    write(&project, "main.rs", b"fn main() {}");
    // A Root file must not be able to opt a secret in (ticket 04): this line is ignored, with a warning.
    write(
        &project,
        ".keepster.toml",
        b"[files]\nsecret_opt_in = [\".env\"]\n",
    );
    daemon.write_global_config(b"[files]\nsecret_opt_in = [\"/.env.example\"]\n");

    let snapshot = take(&daemon, &root).await;

    assert_eq!(
        captured(&daemon, &root, &snapshot).await,
        [".env.example", ".keepster.toml", "main.rs"]
    );

    let admitted = explain(&daemon, &root, ".env.example").await;
    assert_eq!(admitted["captured"], true);
    assert_eq!(
        admitted["rule"]["pattern"], "/.env.example",
        "the opt-in that let it in is named"
    );
    assert_eq!(admitted["rule"]["level"], "global");

    let refused = explain(&daemon, &root, ".env").await;
    assert_eq!(refused["captured"], false);
    assert_eq!(refused["decision"], "secret");
    assert_eq!(refused["rule"]["pattern"], ".env*");
    assert_eq!(refused["rule"]["level"], "default");
    let warnings = refused["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "key_ignored"
                && warning["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("secret_opt_in")),
        "the ignored Root-file key is reported: {warnings:?}"
    );
}

#[tokio::test]
async fn a_file_over_its_cap_is_withheld_and_said_to_be() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    daemon.write_global_config(
        br#"
[files]
max_size = "10B"
max_size_by_pattern = [{ pattern = "*.iso", max = "1KiB" }]
"#,
    );
    write(&project, "small.txt", b"tiny");
    write(&project, "large.txt", b"far more than ten bytes");
    write(&project, "disc.iso", &[0u8; 512]);

    let snapshot = take(&daemon, &root).await;

    assert_eq!(
        captured(&daemon, &root, &snapshot).await,
        ["disc.iso", "small.txt"]
    );
    assert_eq!(
        snapshot["withheld"],
        serde_json::json!([{ "path": "large.txt", "reason": "too_large" }]),
        "the Snapshot names what it deliberately did not hold"
    );
    assert_eq!(
        snapshot["incomplete"], false,
        "withholding is policy, not failure"
    );

    let explained = explain(&daemon, &root, "large.txt").await;
    assert_eq!(explained["decision"], "withheld");
    assert_eq!(explained["reason"], "too_large");
    assert_eq!(explained["size"], 23);
    assert_eq!(explained["max_size"], 10);
    assert_eq!(
        explained["max_size_origin"]["level"], "global",
        "the scalar cap says where it was set"
    );
    assert_eq!(explained["max_size_origin"]["line"], 3);

    let video = explain(&daemon, &root, "disc.iso").await;
    assert_eq!(video["captured"], true);
    assert_eq!(video["max_size"], 1024);
}

#[tokio::test]
async fn explain_names_the_rule_its_level_and_line_and_the_pruning_ancestor() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    daemon.write_global_config(b"[files]\n\nexclude = [\n  \"*.tmp\",\n  \"node_modules/\",\n]\n");
    write(
        &project,
        ".keepster.toml",
        b"[files]\ninclude = [\"node_modules/mylib/**\"]\n",
    );
    write(&project, "node_modules/mylib/index.js", b"vendored");
    write(&project, "scratch.tmp", b"temporary");
    write(&project, "main.rs", b"fn main() {}");

    let excluded = explain(&daemon, &root, "scratch.tmp").await;
    assert_eq!(excluded["captured"], false);
    assert_eq!(excluded["decision"], "excluded");
    assert_eq!(excluded["rule"]["pattern"], "*.tmp");
    assert_eq!(excluded["rule"]["level"], "global");
    assert_eq!(excluded["rule"]["line"], 4);
    assert_eq!(excluded["on_disk"], "file");

    let pruned = explain(&daemon, &root, "node_modules/mylib/index.js").await;
    assert_eq!(pruned["decision"], "pruned");
    assert_eq!(pruned["pruned_by"], "node_modules");
    assert_eq!(pruned["rule"]["pattern"], "node_modules/");
    assert_eq!(pruned["own_rule"]["pattern"], "node_modules/mylib/**");
    assert_eq!(pruned["own_rule"]["effect"], "include");
    assert_eq!(pruned["own_rule"]["level"], "root");
    let warnings = pruned["warnings"].as_array().expect("warnings");
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "negation_unreachable"),
        "the unreachable re-inclusion is warned about: {warnings:?}"
    );

    let kept = explain(&daemon, &root, "main.rs").await;
    assert_eq!(kept["captured"], true);
    assert_eq!(kept["decision"], "captured");
    assert!(kept.get("rule").is_none());

    let reserved = explain(&daemon, &root, ".git/config").await;
    assert_eq!(reserved["decision"], "withheld");
    assert_eq!(reserved["reason"], "reserved_name");
    assert!(reserved.get("on_disk").is_none(), "nothing is there");

    let invalid = daemon
        .get(&format!("/v1/roots/{root}/explain?path=../x"))
        .await;
    assert_eq!(invalid.status, 400);

    // The warning does not make the Root Degraded for a new reason: its state is what it was.
    let listed = daemon.get_json(&format!("/v1/roots/{root}")).await;
    assert!(
        listed.get("state_code").is_none_or(|code| code.is_null()),
        "no reason to give: {listed}"
    );
}

#[tokio::test]
async fn ignore_files_are_honoured_only_on_request_and_beneath_the_users_rules() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(&project, ".gitignore", b"# build output\ndist/\n*.cache\n");
    write(&project, "dist/app.js", b"bundle");
    write(&project, "a.cache", b"cache");
    write(&project, "main.rs", b"fn main() {}");

    let by_default = take(&daemon, &root).await;
    assert_eq!(
        captured(&daemon, &root, &by_default).await,
        [".gitignore", "a.cache", "dist/app.js", "main.rs"],
        "`.gitignore` is an option, not the law"
    );

    daemon.write_global_config(b"[files]\nrespect_gitignore = true\ninclude = [\"a.cache\"]\n");
    let honoured = take(&daemon, &root).await;
    assert_eq!(
        captured(&daemon, &root, &honoured).await,
        [".gitignore", "a.cache", "main.rs"],
        "the user's include outranks the ignore file"
    );

    let explained = explain(&daemon, &root, "dist/app.js").await;
    assert_eq!(explained["rule"]["level"], "ignore-file");
    assert_eq!(explained["rule"]["line"], 2);
}

#[tokio::test]
async fn an_unreadable_configuration_refuses_the_capture_rather_than_dropping_the_rules() {
    // Falling back to defaults would drop the user's exclusions and pull into history exactly what they
    // existed to keep out (ticket 04). The file is broken before the daemon ever reads it, so there is
    // no known-good version to stand in for it either.
    let daemon = Harness::start().await;
    let project = daemon.scratch("project");
    write(
        &project,
        ".keepster.toml",
        b"[files]\nexclude = [\"[unclosed\"]\n",
    );
    let (root, _) = registered(&daemon).await;

    let refused = daemon
        .post(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await;

    assert_eq!(refused.status, 409);
    let problem: Value = serde_json::from_slice(&refused.body).expect("a problem");
    assert_eq!(problem["code"], "config_invalid");

    write(&project, ".keepster.toml", b"[files]\nexlude = [\"x\"]\n");
    let typo = daemon
        .post(
            &format!("/v1/roots/{root}/snapshots"),
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(typo.status, 409, "a misspelt key is not silently ignored");
}

#[tokio::test]
async fn keepster_explain_answers_for_a_path_on_this_machine() {
    let daemon = Harness::start().await;
    let (_, project) = registered(&daemon).await;
    write(
        &project,
        ".keepster.toml",
        b"[files]\nexclude = [\"*.log\"]\n",
    );
    write(&project, "logs/today.log", b"noise");
    let inside = project.join("logs");

    let run = daemon
        .run_cli_in(&inside, KEEPSTER, &["explain", "today.log"])
        .await;

    assert_eq!(run.exit_code, Some(0), "stderr: {}", run.stderr);
    let printed = String::from_utf8(run.stdout).expect("text");
    assert!(printed.contains("not captured"), "in:\n{printed}");
    assert!(printed.contains("*.log"), "in:\n{printed}");
    assert!(printed.contains("root"), "in:\n{printed}");

    let json = daemon
        .run_cli_in(&inside, KEEPSTER, &["explain", "today.log", "--json"])
        .await;
    let parsed: Value = serde_json::from_slice(&json.stdout).expect("the daemon's own JSON");
    assert_eq!(parsed["decision"], "excluded");
}

#[tokio::test]
async fn a_cap_can_be_written_for_a_directory_and_lifted_by_a_negation() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    daemon.write_global_config(
        br#"
[files]
max_size = "4B"
max_size_by_pattern = [
  { pattern = "media/", max = "1KiB" },
  { pattern = "!media/small/" },
]
"#,
    );
    write(&project, "media/clip.bin", b"a short clip");
    write(&project, "media/small/note.txt", b"too long for four bytes");

    let snapshot = take(&daemon, &root).await;

    assert_eq!(
        captured(&daemon, &root, &snapshot).await,
        ["media/clip.bin"]
    );
    assert_eq!(
        snapshot["withheld"],
        serde_json::json!([{ "path": "media/small/note.txt", "reason": "too_large" }]),
        "the negation returns its files to the scalar cap"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn explain_never_calls_a_link_captured_while_the_walk_passes_links_over() {
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(&project, "target.txt", b"content");
    std::os::unix::fs::symlink("target.txt", project.join("link.txt")).expect("a symlink");

    let explained = explain(&daemon, &root, "link.txt").await;

    assert_eq!(explained["captured"], false);
    assert_eq!(explained["decision"], "withheld");
    assert_eq!(explained["reason"], "symlink");
    assert_eq!(explained["on_disk"], "symlink");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn an_excluded_directory_is_never_listed_at_all() {
    use std::os::unix::ffi::OsStrExt;

    // A name that is not UTF-8 is reported as unreadable by any walk that meets it (until ticket 48),
    // so meeting one here would make the Snapshot incomplete. It sits inside an excluded directory,
    // and the Snapshot stays whole only if the walk never listed that directory.
    let daemon = Harness::start().await;
    let (root, project) = registered(&daemon).await;
    write(
        &project,
        ".keepster.toml",
        b"[files]\nexclude = [\"vendor/\"]\n",
    );
    write(&project, "main.rs", b"fn main() {}");
    std::fs::create_dir_all(project.join("vendor")).expect("the excluded directory");
    let odd = std::ffi::OsStr::from_bytes(b"\xff.bin");
    std::fs::write(project.join("vendor").join(odd), b"x").expect("an odd name");

    let snapshot = take(&daemon, &root).await;

    assert_eq!(
        snapshot["incomplete"], false,
        "the excluded directory was entered: {snapshot}"
    );
}
