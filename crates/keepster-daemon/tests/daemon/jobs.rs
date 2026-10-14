//! Jobs through `/v1` (ticket 54): a manual Maintenance Pass is accepted with `202` and a job's identity,
//! announced as it starts and ends, and queryable afterwards with how it ended.

use std::time::Duration;

use keepster_testkit::Harness;
use serde_json::{Value, json};

fn text(response: &keepster_cli::Response) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

fn json(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

/// A watched Root with one Snapshot.
pub(crate) async fn captured(daemon: &Harness) -> String {
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.txt"), b"one").expect("a file");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await;

    root
}

#[tokio::test]
async fn a_manual_maintenance_pass_is_a_job_announced_and_queryable() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let root = captured(&daemon).await;
    let mut stream = daemon.events(None).await;

    let accepted = daemon
        .post(&format!("/v1/roots/{root}/maintenance"), &json!({}))
        .await;

    assert_eq!(accepted.status.as_u16(), 202, "{}", text(&accepted));
    let job = json(&accepted)["job_id"]
        .as_str()
        .expect("a job id")
        .to_owned();
    assert_eq!(
        accepted
            .headers
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some(format!("/v1/jobs/{job}").as_str())
    );

    let mut seen = Vec::new();
    while !seen.contains(&"job.completed".to_owned()) {
        let event = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("the job ends within the timeout")
            .expect("the stream reads")
            .expect("open");
        let data: Value = serde_json::from_str(&event.data).expect("JSON");
        if event.event.starts_with("job.") {
            assert_eq!(data["job"], job.as_str(), "{data}");
        }
        if event.event == "job.completed" {
            assert_eq!(data["state"], "succeeded", "{data}");
            assert_eq!(data["kind"], "maintenance", "{data}");
            assert_eq!(data["root"], root.as_str(), "{data}");
        }
        seen.push(event.event);
    }
    let started = seen.iter().position(|name| name == "job.started");
    let maintained = seen.iter().position(|name| name == "maintenance.completed");
    assert!(
        started.is_some() && maintained.is_some() && started < maintained,
        "started, then the pass, then the end: {seen:?}"
    );

    let ended = daemon.get_json(&format!("/v1/jobs/{job}")).await;
    assert_eq!(ended["id"], job.as_str(), "{ended}");
    assert_eq!(ended["kind"], "maintenance");
    assert_eq!(ended["root"], root.as_str());
    assert_eq!(ended["state"], "succeeded");
    assert!(
        ended["result"]["reclaimed_bytes"].is_u64(),
        "how much it reclaimed: {ended}"
    );
    assert!(ended.get("error").is_none(), "{ended}");
}

#[tokio::test]
async fn a_job_nobody_started_is_not_found() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;

    let answer = daemon.get("/v1/jobs/01K6DX3QR7M2V8C9WZ4PN5YQ1A").await;

    assert_eq!(answer.status.as_u16(), 404);
    assert_eq!(json(&answer)["code"], "job_not_found", "{}", text(&answer));
}

#[tokio::test]
async fn without_a_usable_git_maintenance_is_refused_at_the_door() {
    let daemon = Harness::start_with_global_config(
        format!(
            "{}\n[git]\npath = {}\n",
            String::from_utf8_lossy(keepster_testkit::MANUAL),
            toml_string(
                &std::env::temp_dir()
                    .join("keepster-no-such-git")
                    .display()
                    .to_string()
            )
        )
        .as_bytes(),
    )
    .await;
    let root = captured(&daemon).await;

    let refused = daemon
        .post(&format!("/v1/roots/{root}/maintenance"), &json!({}))
        .await;

    assert_eq!(refused.status.as_u16(), 409, "{}", text(&refused));
    assert_eq!(json(&refused)["code"], "maintenance_unavailable");
}

#[tokio::test]
async fn maintaining_a_root_nobody_watched_is_not_found() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;

    let answer = daemon
        .post(
            "/v1/roots/01K6DX3QR7M2V8C9WZ4PN5YQ1A/maintenance",
            &json!({}),
        )
        .await;

    assert_eq!(answer.status.as_u16(), 404);
    assert_eq!(json(&answer)["code"], "root_not_found");
}

/// Linux's "text file busy": a script still open for writing in a child another test forked.
#[cfg(unix)]
const ETXTBSY: i32 = 26;

/// `text` as a TOML basic string, so a Windows path's backslashes survive.
fn toml_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\"))
}

/// Where [`whose_git_fails`] puts the `git` it configures.
#[cfg(unix)]
pub(crate) fn fake_git(daemon: &Harness) -> std::path::PathBuf {
    daemon.scratch("fake-git").join("git")
}

/// Put an executable shell script saying `body` at `path`, replacing whatever is there by a rename —
/// never by writing into a file a pass may be running — and wait until it can be run.
#[cfg(unix)]
pub(crate) fn install_script(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    let written = path.with_extension("new");
    std::fs::write(&written, body).expect("a script");
    std::fs::set_permissions(&written, std::fs::Permissions::from_mode(0o755)).expect("executable");
    std::fs::rename(&written, path).expect("in place");
    // A file just written may still be open in a child another test forked (ETXTBSY): wait it out.
    for _ in 0..200 {
        match std::process::Command::new(path).arg("--version").output() {
            Err(error) if error.raw_os_error() == Some(ETXTBSY) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => break,
        }
    }
}

/// A daemon whose configured `git` passes its check and fails every pass, saying `the disk said no` on
/// its error stream — already found, so a pass asked for runs it at once.
#[cfg(unix)]
pub(crate) async fn whose_git_fails() -> Harness {
    // A look a second, so the `git.path` written below is picked up at once; no pass is due meanwhile.
    let quick = keepster_daemon::HousekeepingTiming {
        every: Duration::from_secs(1),
        quiet: Duration::from_secs(3600),
        ceiling: Duration::from_secs(3600),
        maintain_every: Duration::from_secs(24 * 3600),
    };
    let daemon = Harness::start_with_housekeeping(quick, keepster_testkit::MANUAL).await;
    let script = fake_git(&daemon);
    install_script(
        &script,
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'git version 2.43.0'; exit 0; fi\n\
         echo 'fatal: the disk said no' >&2\nexit 128\n",
    );
    daemon.write_global_config(
        format!(
            "{}\n[git]\npath = {}\n",
            String::from_utf8_lossy(keepster_testkit::MANUAL),
            toml_string(&script.display().to_string())
        )
        .as_bytes(),
    );
    tokio::time::timeout(Duration::from_secs(30), async {
        while daemon.get_json("/v1/status").await["daemon"]["git"]["path"]
            != script.display().to_string()
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the configured git is found at the next look");
    daemon
}

#[cfg(unix)]
#[tokio::test]
async fn a_pass_that_fails_ends_its_job_failed_with_what_git_said() {
    let daemon = whose_git_fails().await;
    let root = captured(&daemon).await;
    let mut stream = daemon.events(None).await;

    let job = daemon
        .post_json(&format!("/v1/roots/{root}/maintenance"), &json!({}))
        .await["job_id"]
        .as_str()
        .expect("a job id")
        .to_owned();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("the job ends within the timeout")
            .expect("the stream reads")
            .expect("open");
        if event.event == "job.completed" {
            let data: Value = serde_json::from_str(&event.data).expect("JSON");
            assert_eq!(data["state"], "failed", "{data}");
            break;
        }
    }

    let ended = daemon.get_json(&format!("/v1/jobs/{job}")).await;
    assert_eq!(ended["state"], "failed", "{ended}");
    assert_eq!(ended["error"]["code"], "maintenance_failed", "{ended}");
    assert!(
        ended["error"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("the disk said no")),
        "{ended}"
    );
    assert!(ended.get("result").is_none(), "{ended}");
}

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn the_cli_follows_a_job_to_its_end_or_detaches_with_its_identity() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let root = captured(&daemon).await;
    let project = daemon.scratch("project");

    let followed = daemon.run_cli_in(&project, KEEPSTER, &["maintain"]).await;
    assert_eq!(followed.exit_code, Some(0), "{}", followed.stderr);
    assert!(followed.stderr.contains("queued"), "{}", followed.stderr);
    assert!(followed.stderr.contains("running"), "{}", followed.stderr);
    let said = String::from_utf8_lossy(&followed.stdout);
    assert!(said.contains("succeeded"), "{said}");

    let detached = daemon
        .run_cli(KEEPSTER, &["maintain", "-r", &root, "--detach"])
        .await;
    assert_eq!(detached.exit_code, Some(0), "{}", detached.stderr);
    let job = String::from_utf8_lossy(&detached.stdout).trim().to_owned();
    assert_eq!(job.len(), 26, "only the job's identity: {job}");
    let reported = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let run = daemon.run_cli(KEEPSTER, &["job", &job, "--json"]).await;
            assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
            let job: Value = serde_json::from_slice(&run.stdout).expect("JSON");
            if job["state"] == "succeeded" {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("the detached job ends");
    assert_eq!(reported["kind"], "maintenance");

    let unknown = daemon
        .run_cli(KEEPSTER, &["job", "01K6DX3QR7M2V8C9WZ4PN5YQ1A"])
        .await;
    assert_eq!(unknown.exit_code, Some(4), "{}", unknown.stderr);
}

#[tokio::test]
async fn the_cli_refused_for_want_of_a_git_exits_as_status_does() {
    let daemon = Harness::start_with_global_config(
        format!(
            "{}\n[git]\npath = {}\n",
            String::from_utf8_lossy(keepster_testkit::MANUAL),
            toml_string(
                &std::env::temp_dir()
                    .join("keepster-no-such-git")
                    .display()
                    .to_string()
            )
        )
        .as_bytes(),
    )
    .await;
    let root = captured(&daemon).await;

    let refused = daemon.run_cli(KEEPSTER, &["maintain", "-r", &root]).await;

    assert_eq!(refused.exit_code, Some(11), "{}", refused.stderr);
}

#[tokio::test]
async fn the_history_of_a_root_no_longer_watched_can_be_maintained_too() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let root = captured(&daemon).await;
    assert_eq!(
        daemon
            .delete(&format!("/v1/roots/{root}"))
            .await
            .status
            .as_u16(),
        204
    );

    let accepted = daemon
        .post(&format!("/v1/roots/{root}/maintenance"), &json!({}))
        .await;

    assert_eq!(accepted.status.as_u16(), 202, "{}", text(&accepted));
    let job = json(&accepted)["job_id"]
        .as_str()
        .expect("a job id")
        .to_owned();
    let ended = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let job = daemon.get_json(&format!("/v1/jobs/{job}")).await;
            if job["state"] == "succeeded" || job["state"] == "failed" {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the job ends");
    assert_eq!(ended["state"], "succeeded", "{ended}");
}
