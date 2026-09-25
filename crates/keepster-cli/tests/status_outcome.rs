//! What `keepster status` prints and returns, for every answer the daemon can give.
//!
//! These assertions are over values because several of the branches cannot be produced by a healthy
//! daemon at all — a refused request, a payload this build cannot parse, a Root that is not
//! `Watching` before any Root can be registered. Ticket 37 adds the harness-level assertion for the
//! non-zero code once Roots exist.

use hyper::StatusCode;
use keepster_api::{DaemonStatus, Reported, Root, RootState, Status, VersionInfo};
use keepster_cli::{ClientError, Response, outcome};

fn status_with(roots: Vec<Root>) -> Status {
    Status {
        daemon: DaemonStatus {
            version: VersionInfo {
                build_version: "0.0.0".to_owned(),
                build_hash: "unknown".to_owned(),
                api_version: "v1".to_owned(),
            },
            pid: 1,
            uptime_seconds: 0,
            autostart: Reported::Unknown,
            linger: Reported::Unknown,
            maintenance: Reported::Unknown,
        },
        roots,
    }
}

fn root(id: &str, state: RootState) -> Root {
    Root {
        id: id.to_owned(),
        path: format!("/home/someone/projects/{}", id.to_lowercase()),
        state,
        state_code: (state != RootState::Watching).then(|| "capture_unavailable".to_owned()),
    }
}

fn answered(status: StatusCode, body: Vec<u8>) -> Result<Response, ClientError> {
    Ok(Response { status, body })
}

fn served(status: &Status) -> Result<Response, ClientError> {
    answered(
        StatusCode::OK,
        serde_json::to_vec(status).expect("the payload serialises"),
    )
}

#[test]
fn an_empty_registry_is_success() {
    let result = outcome(served(&status_with(Vec::new())), false);

    assert_eq!(result.code, 0);
    assert_eq!(result.stderr, None);
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("no watched roots"),
        "an empty Registry must be stated, not left blank"
    );
}

#[test]
fn every_root_watching_is_success() {
    let status = status_with(vec![
        root("01ARZ3NDEKTSV4RRFFQ69G5FAV", RootState::Watching),
        root("01BX5ZZKBKACTAV9WEVGEMMVRZ", RootState::Watching),
    ]);

    let result = outcome(served(&status), false);

    assert_eq!(result.code, 0);
    let printed = String::from_utf8_lossy(&result.stdout);
    // Ticket 07 names a Root the way a person recognises it: the directory's own name plus the first
    // eight characters of the identity. The full ULID is what URLs and scripts use.
    assert!(printed.contains("01ARZ3ND"), "{printed}");
    assert!(
        printed.contains("01arz3ndektsv4rrffq69g5fav"),
        "the path must be shown too:\n{printed}"
    );
    assert!(printed.contains("watching"), "{printed}");
}

#[test]
fn one_degraded_root_among_healthy_ones_is_reported() {
    let status = status_with(vec![
        root("01ARZ3NDEKTSV4RRFFQ69G5FAV", RootState::Watching),
        root("01BX5ZZKBKACTAV9WEVGEMMVRZ", RootState::Degraded),
    ]);

    let result = outcome(served(&status), false);

    assert_eq!(result.code, 10);
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("degraded"),
        "the state must be named in the output, not only in the exit code"
    );
}

#[test]
fn an_unavailable_root_is_reported() {
    let status = status_with(vec![root(
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        RootState::Unavailable,
    )]);

    let result = outcome(served(&status), false);

    assert_eq!(result.code, 10);
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("unavailable"),
        "the state must be named in the output"
    );
}

#[test]
fn an_unreachable_daemon_names_the_control_channel_and_exits_three() {
    let error = ClientError::Unreachable {
        path: "/run/keepster/daemon.sock".into(),
        source: std::io::Error::from(std::io::ErrorKind::NotFound),
    };

    let result = outcome(Err(error), false);

    assert_eq!(result.code, 3);
    assert!(result.stdout.is_empty());
    assert!(
        result
            .stderr
            .as_deref()
            .expect("a message")
            .contains("/run/keepster/daemon.sock"),
        "the failure must name what it tried"
    );
}

#[test]
fn a_malformed_request_is_not_mistaken_for_a_dead_daemon() {
    // A request the CLI itself built wrongly is a defect in the CLI, not an absent daemon, so it must
    // not claim the daemon is unreachable — a script would retry forever.
    let broken = hyper::Request::builder()
        .method("not a method")
        .body(())
        .expect_err("an invalid method is refused by the builder");

    let result = outcome(Err(ClientError::Request(broken)), false);

    assert_eq!(result.code, 1);
    assert!(result.stdout.is_empty());
}

#[test]
fn a_refused_request_reports_the_status_and_prints_nothing() {
    let result = outcome(
        answered(StatusCode::INTERNAL_SERVER_ERROR, Vec::new()),
        false,
    );

    assert_eq!(result.code, 1);
    assert!(result.stdout.is_empty());
    assert!(
        result.stderr.as_deref().expect("a message").contains("500"),
        "the daemon's own status must reach the user"
    );
}

#[test]
fn a_payload_this_build_cannot_read_is_still_shown_verbatim() {
    let body = br#"{"daemon":"a shape this version has never seen"}"#.to_vec();

    let result = outcome(answered(StatusCode::OK, body.clone()), false);

    assert_eq!(
        result.stdout, body,
        "hiding the daemon's answer is the opposite of what diagnosing it needs"
    );
    assert_eq!(
        result.code, 1,
        "health cannot be claimed from an unread payload"
    );
    assert!(result.stderr.is_some());
}

#[test]
fn json_prints_the_payload_unchanged_including_fields_this_build_does_not_know() {
    // A CLI that parsed and re-serialised would drop `invented_later` and normalise the spacing.
    let body = br#"{"daemon":{"api_version":"v1","invented_later":true},  "roots":[]}"#.to_vec();

    let result = outcome(answered(StatusCode::OK, body.clone()), true);

    assert_eq!(result.stdout, body);
}

#[test]
fn json_and_human_forms_agree_on_the_exit_code() {
    let status = status_with(vec![root(
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        RootState::Degraded,
    )]);

    let as_json = outcome(served(&status), true);
    let as_text = outcome(served(&status), false);

    assert_eq!(as_json.code, as_text.code);
    assert_ne!(
        as_json.stdout, as_text.stdout,
        "the two forms print differently even though they report the same health"
    );
}
