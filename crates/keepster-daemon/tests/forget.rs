//! Forget through `/v1` (ticket 81): a request carrying everything it must, refused at the door when it
//! does not, run as a job that reports its progress and what it removed, the Working Tree untouched —
//! and, asked for, the path excluded from now on at a level of its own.

use std::time::Duration;

use keepster_testkit::Harness;
use serde_json::{Value, json};

fn text(response: &keepster_cli::Response) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

fn json(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("JSON")
}

/// A watched Root holding `notes.txt` and `secret.txt`, captured once; its id and its directory.
async fn leaked(daemon: &Harness) -> (String, std::path::PathBuf, String) {
    let project = daemon.scratch("project");
    std::fs::write(project.join("notes.txt"), b"kept").expect("a file");
    std::fs::write(project.join("secret.txt"), b"hunter2").expect("a file");
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

/// Ask for a Forget and follow its job to its end: every event of it, and how it ended.
async fn forgotten(daemon: &Harness, root: &str, body: &Value) -> (Vec<(String, Value)>, Value) {
    let mut stream = daemon.events(None).await;
    let accepted = daemon.post(&format!("/v1/roots/{root}/forget"), body).await;
    assert_eq!(accepted.status.as_u16(), 202, "{}", text(&accepted));
    let job = json(&accepted)["job_id"]
        .as_str()
        .expect("a job id")
        .to_owned();

    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(30), stream.next())
            .await
            .expect("the job ends within the timeout")
            .expect("the stream reads")
            .expect("open");
        let data: Value = serde_json::from_str(&event.data).expect("JSON");
        if data["job"] != job.as_str() {
            continue;
        }
        let done = event.event == "job.completed";
        seen.push((event.event, data));
        if done {
            break;
        }
    }

    (seen, daemon.get_json(&format!("/v1/jobs/{job}")).await)
}

#[tokio::test]
async fn forgetting_a_path_removes_every_version_and_says_what_is_still_on_disk() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project, snapshot) = leaked(&daemon).await;

    let (events, job) = forgotten(
        &daemon,
        &root,
        &json!({ "glob": "secret.txt", "confirm": true, "on_exclude": "keep-watching" }),
    )
    .await;

    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(job["kind"], "forget");
    let result = &job["result"];
    assert_eq!(result["snapshots_touched"], 1, "{result}");
    assert_eq!(result["blobs_removed"], 1, "{result}");
    assert_eq!(result["removed_paths"], json!(["secret.txt"]), "{result}");
    assert_eq!(result["excluded"], false, "{result}");
    assert_eq!(
        result["still_on_disk"],
        json!(["secret.txt"]),
        "watched still, the next Snapshot would take it back: {result}"
    );
    assert!(
        result["reclaimed_bytes"].is_u64(),
        "it ended with a pass: {result}"
    );
    assert!(
        events
            .iter()
            .any(|(name, data)| name == "job.progress" && data["done"] == data["total"]),
        "it counted its steps: {events:?}"
    );

    let read = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{snapshot}/content?path=secret.txt"
        ))
        .await;
    assert_eq!(read.status.as_u16(), 404, "{}", text(&read));
    let timeline = daemon
        .get_json(&format!("/v1/roots/{root}/snapshots"))
        .await;
    assert_eq!(
        timeline["snapshots"][0]["id"],
        snapshot.as_str(),
        "{timeline}"
    );
    assert_eq!(timeline["snapshots"][0]["forgotten_paths"], 1, "{timeline}");
    assert_eq!(
        std::fs::read(project.join("secret.txt")).expect("untouched"),
        b"hunter2",
        "the Working Tree is the user's"
    );
}

#[tokio::test]
async fn a_request_missing_anything_it_must_carry_is_refused_at_the_door() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, _, _) = leaked(&daemon).await;

    for body in [
        json!({ "confirm": true, "on_exclude": "exclude" }),
        json!({ "glob": "secret.txt", "on_exclude": "exclude" }),
        json!({ "glob": "secret.txt", "confirm": false, "on_exclude": "exclude" }),
        json!({ "glob": "secret.txt", "confirm": true }),
        json!({ "glob": "secret.txt", "confirm": true, "on_exclude": "maybe" }),
    ] {
        let refused = daemon
            .post(&format!("/v1/roots/{root}/forget"), &body)
            .await;
        assert_eq!(refused.status.as_u16(), 400, "{body}: {}", text(&refused));
        assert_eq!(
            json(&refused)["code"],
            "confirmation_required",
            "{body}: {}",
            text(&refused)
        );
    }
    let refused = daemon
        .post(
            &format!("/v1/roots/{root}/forget"),
            &json!({ "glob": "!secret.txt", "confirm": true, "on_exclude": "exclude" }),
        )
        .await;
    assert_eq!(refused.status.as_u16(), 400, "{}", text(&refused));
    assert_eq!(json(&refused)["code"], "glob_invalid", "{}", text(&refused));
}

#[tokio::test]
async fn excluding_keeps_the_path_out_from_now_on_at_a_level_of_its_own() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project, _) = leaked(&daemon).await;
    // The user's own file re-includes it; Forget's exclusion outranks that.
    std::fs::write(
        project.join(".keepster.toml"),
        b"[files]\ninclude = [\"secret.txt\"]\n",
    )
    .expect("a file");

    let (_, job) = forgotten(
        &daemon,
        &root,
        &json!({ "glob": "secret.txt", "confirm": true, "on_exclude": "exclude" }),
    )
    .await;
    assert_eq!(job["state"], "succeeded", "{job}");

    let explained = daemon
        .get_json(&format!("/v1/roots/{root}/explain?path=secret.txt"))
        .await;
    assert_eq!(explained["rule"]["level"], "forget", "{explained}");
    let config = daemon
        .get_json(&format!("/v1/roots/{root}/config"))
        .await
        .to_string();
    assert!(
        config.contains(r#""level":"forget""#) && config.contains("secret.txt"),
        "`config` names it at its own level: {config}"
    );
    let next = daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await;
    let held = daemon
        .get(&format!(
            "/v1/roots/{root}/snapshots/{}/content?path=secret.txt",
            next["id"].as_str().expect("a Snapshot")
        ))
        .await;
    assert_eq!(held.status.as_u16(), 404, "not captured again");
}

#[tokio::test]
async fn a_path_opted_in_past_the_denylist_cannot_be_excluded() {
    let daemon = Harness::start_with_global_config(
        format!(
            "{}\n[files]\nsecret_opt_in = [\".env\"]\n",
            String::from_utf8_lossy(keepster_testkit::MANUAL)
        )
        .as_bytes(),
    )
    .await;
    let project = daemon.scratch("project");
    std::fs::write(project.join(".env"), b"TOKEN=1").expect("a file");
    let root = daemon
        .post_json("/v1/roots", &json!({ "path": project }))
        .await["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    daemon
        .post_json(&format!("/v1/roots/{root}/snapshots"), &json!({}))
        .await;

    let refused = daemon
        .post(
            &format!("/v1/roots/{root}/forget"),
            &json!({ "glob": ".env", "confirm": true, "on_exclude": "exclude" }),
        )
        .await;

    assert_eq!(refused.status.as_u16(), 409, "{}", text(&refused));
    assert_eq!(
        json(&refused)["code"],
        "secret_opted_in",
        "{}",
        text(&refused)
    );
    let (_, job) = forgotten(
        &daemon,
        &root,
        &json!({ "glob": ".env", "confirm": true, "on_exclude": "keep-watching" }),
    )
    .await;
    assert_eq!(
        job["state"], "succeeded",
        "keeping it watched is allowed: {job}"
    );
}

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn the_cli_refuses_without_everything_and_then_says_the_secret_is_still_on_disk() {
    let daemon = Harness::start_with_global_config(keepster_testkit::MANUAL).await;
    let (root, project, snapshot) = leaked(&daemon).await;
    let url = format!("/v1/roots/{root}/snapshots/{snapshot}/content?path=secret.txt");
    let still_there = || daemon.get(&url);

    // Not a terminal, so nothing can be asked: whatever is missing is refused, and nothing happens.
    for args in [
        vec!["forget", "secret.txt", "--keep-watching"],
        vec!["forget", "secret.txt", "--yes"],
    ] {
        let refused = daemon.run_cli_in(&project, KEEPSTER, &args).await;
        assert_eq!(refused.exit_code, Some(2), "{args:?}: {}", refused.stderr);
        assert_eq!(still_there().await.status.as_u16(), 200, "{args:?}");
    }

    let run = daemon
        .run_cli_in(
            &project,
            KEEPSTER,
            &["forget", "secret.txt", "--keep-watching", "--yes"],
        )
        .await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    assert_eq!(still_there().await.status.as_u16(), 404, "forgotten");
    let said = format!("{}{}", String::from_utf8_lossy(&run.stdout), run.stderr);
    assert!(said.contains("1 Snapshot"), "{said}");
    assert!(said.contains("still on disk"), "{said}");
    assert!(
        said.contains("rotat"),
        "it says the secret still needs rotating: {said}"
    );
    assert!(
        said.contains("the next Snapshot"),
        "kept watched, it says what happens next: {said}"
    );
}
