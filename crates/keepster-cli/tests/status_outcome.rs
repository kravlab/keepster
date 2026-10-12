//! What `keepster status` prints and returns, for every answer the daemon can give.
//!
//! These assertions are over values because several of the branches cannot be produced by a healthy
//! daemon at all — a refused request, a payload this build cannot parse, a Root that is not
//! `Watching` before any Root can be registered. Ticket 37 adds the harness-level assertion for the
//! non-zero code once Roots exist.

use hyper::StatusCode;
use keepster_api::{
    DaemonStatus, GitStatus, InstallHint, Reported, Root, RootState, Status, StoreFormat,
    VersionInfo,
};
use keepster_cli::{ClientError, Response, outcome};

fn status_with(roots: Vec<Root>) -> Status {
    Status {
        daemon: DaemonStatus {
            version: VersionInfo {
                build_version: "0.0.0".to_owned(),
                build_hash: "unknown".to_owned(),
                api_version: "v1".to_owned(),
                store_format: Some(StoreFormat {
                    container: 1,
                    content: 1,
                    sidecar: 1,
                    index: 5,
                }),
            },
            pid: 1,
            uptime_seconds: 0,
            autostart: Reported::Unknown,
            linger: Reported::Unknown,
            maintenance: Reported::Available,
            git: GitStatus {
                path: Some("/usr/bin/git".to_owned()),
                version: Some("2.43.0".to_owned()),
                minimum: "2.32.0".to_owned(),
                reason: None,
                detail: None,
                install: None,
            },
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
        pending_paths: 0,
        oldest_pending_seconds: None,
        follow_targets: None,
        unrecognized_records: None,
    }
}

fn answered(status: StatusCode, body: Vec<u8>) -> Result<Response, ClientError> {
    Ok(Response {
        status,
        headers: Default::default(),
        body,
    })
}

fn served(status: &Status) -> Result<Response, ClientError> {
    answered(
        StatusCode::OK,
        serde_json::to_vec(status).expect("the payload serialises"),
    )
}

#[test]
fn an_empty_registry_is_success() {
    let result = outcome(served(&status_with(Vec::new())), false, None);

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

    let result = outcome(served(&status), false, None);

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

    let result = outcome(served(&status), false, None);

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

    let result = outcome(served(&status), false, None);

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

    let result = outcome(Err(error), false, None);

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

    let result = outcome(Err(ClientError::Request(broken)), false, None);

    assert_eq!(result.code, 1);
    assert!(result.stdout.is_empty());
}

#[test]
fn a_refused_request_reports_the_status_and_prints_nothing() {
    let result = outcome(
        answered(StatusCode::INTERNAL_SERVER_ERROR, Vec::new()),
        false,
        None,
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

    let result = outcome(answered(StatusCode::OK, body.clone()), false, None);

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

    let result = outcome(answered(StatusCode::OK, body.clone()), true, None);

    assert_eq!(result.stdout, body);
}

#[test]
fn json_and_human_forms_agree_on_the_exit_code() {
    let status = status_with(vec![root(
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        RootState::Degraded,
    )]);

    let as_json = outcome(served(&status), true, None);
    let as_text = outcome(served(&status), false, None);

    assert_eq!(as_json.code, as_text.code);
    assert_ne!(
        as_json.stdout, as_text.stdout,
        "the two forms print differently even though they report the same health"
    );
}

#[test]
fn without_a_git_maintenance_runs_with_status_says_how_to_install_one_and_is_not_success() {
    let mut status = status_with(vec![root(
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        RootState::Watching,
    )]);
    status.daemon.maintenance = Reported::Unavailable;
    status.daemon.git = GitStatus {
        path: None,
        version: None,
        minimum: "2.32.0".to_owned(),
        reason: Some("missing".to_owned()),
        detail: None,
        install: Some(InstallHint {
            command: Some("sudo apt install git".to_owned()),
            note: None,
            link: "https://git-scm.com/downloads".to_owned(),
        }),
    };

    let result = outcome(served(&status), false, None);

    assert_eq!(
        result.code, 11,
        "a store that only grows is not a healthy one"
    );
    let printed = String::from_utf8_lossy(&result.stdout);
    assert!(printed.contains("maintenance  unavailable"), "{printed}");
    assert!(printed.contains("no git was found"), "{printed}");
    assert!(
        printed.contains("install it: sudo apt install git"),
        "{printed}"
    );

    status.roots[0].state = RootState::Degraded;
    assert_eq!(
        outcome(served(&status), false, None).code,
        10,
        "a Root not watched is the graver news"
    );
}

#[test]
fn the_store_format_is_a_labelled_row_beside_the_build_and_the_api() {
    let result = outcome(served(&status_with(Vec::new())), false, None);

    let printed = String::from_utf8_lossy(&result.stdout);
    assert!(printed.contains("  api          v1"), "{printed}");
    assert!(
        printed.contains("  store format container 1, content 1, sidecar 1, index 5"),
        "{printed}"
    );
}

#[test]
fn a_daemon_that_does_not_say_its_store_format_reads_as_unknown_not_as_zero() {
    let mut status = status_with(Vec::new());
    status.daemon.version.store_format = None;

    let printed =
        String::from_utf8_lossy(&outcome(served(&status), false, None).stdout).into_owned();

    assert!(printed.contains("  store format unknown"), "{printed}");
}

#[test]
fn records_a_newer_keepster_wrote_are_named_under_their_root() {
    let mut watched = root("01J0000000000000000000000A", RootState::Watching);
    watched.unrecognized_records = Some(2);

    let result = outcome(served(&status_with(vec![watched])), false, None);

    let printed = String::from_utf8_lossy(&result.stdout);
    assert!(
        printed.contains("2 metadata record(s) from a newer Keepster are not understood"),
        "{printed}"
    );
    assert_eq!(result.code, 0, "the Root is watched as ever");
}

#[test]
fn a_root_whose_disk_is_gone_says_its_history_still_reads_and_a_held_one_says_how_to_record_it() {
    let mut gone = root("01J0000000000000000000000A", RootState::Unavailable);
    gone.state_code = Some("volume_mismatch".to_owned());
    let mut held = root("01J0000000000000000000000B", RootState::Degraded);
    held.state_code = Some("capture_held".to_owned());

    let result = outcome(served(&status_with(vec![gone, held])), false, None);

    let printed = String::from_utf8_lossy(&result.stdout);
    assert!(printed.contains("its history reads as ever"), "{printed}");
    assert!(
        printed.contains("keepster snapshot -r 01J0000000000000000000000B"),
        "{printed}"
    );
    assert_eq!(
        result.code, 10,
        "a Root that cannot be captured is the graver news"
    );
}

#[test]
fn pending_changes_are_a_count_and_an_age_never_a_listing() {
    let mut busy = root("01J0000000000000000000000A", RootState::Watching);
    busy.pending_paths = 3;
    busy.oldest_pending_seconds = Some(12);

    let result = outcome(served(&status_with(vec![busy])), false, None);

    let printed = String::from_utf8_lossy(&result.stdout);
    assert!(
        printed.contains("3 changes pending, the oldest 12s ago"),
        "{printed}"
    );
}

#[test]
fn the_git_in_use_is_shown_with_its_version() {
    let result = outcome(served(&status_with(Vec::new())), false, None);

    let printed = String::from_utf8_lossy(&result.stdout);
    assert!(printed.contains("maintenance  available"), "{printed}");
    assert!(
        printed.contains("git          2.43.0 at /usr/bin/git"),
        "{printed}"
    );
}

#[test]
fn a_git_the_shell_finds_is_offered_as_a_line_to_paste_never_written() {
    let mut status = status_with(Vec::new());
    status.daemon.maintenance = Reported::Unavailable;
    status.daemon.git = GitStatus {
        path: None,
        version: None,
        minimum: "2.32.0".to_owned(),
        reason: Some("missing".to_owned()),
        detail: None,
        install: None,
    };

    let result = outcome(
        served(&status),
        false,
        Some(std::path::Path::new("/opt/tools/bin/git")),
    );

    let printed = String::from_utf8_lossy(&result.stdout);
    assert!(printed.contains("[git]"), "{printed}");
    assert!(printed.contains("path = '/opt/tools/bin/git'"), "{printed}");
    assert_eq!(result.code, 11);
}

#[test]
fn a_failed_pass_shows_what_git_said_and_how_it_is_looked_for_again() {
    let mut status = status_with(Vec::new());
    status.daemon.maintenance = Reported::Unavailable;
    status.daemon.git.reason = Some("failed".to_owned());
    status.daemon.git.detail = Some("fatal: bad object HEAD".to_owned());

    let result = outcome(
        served(&status),
        false,
        Some(std::path::Path::new("/opt/tools/bin/git")),
    );

    let printed = String::from_utf8_lossy(&result.stdout);
    assert!(
        printed.contains("git          2.43.0 at /usr/bin/git"),
        "{printed}"
    );
    assert!(
        printed.contains("git said: fatal: bad object HEAD"),
        "{printed}"
    );
    assert!(
        printed.contains("until the daemon restarts or git.path changes"),
        "{printed}"
    );
    assert!(
        !printed.contains("[git]"),
        "the git that failed is the one configured or found: {printed}"
    );
}
