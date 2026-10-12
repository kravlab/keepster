//! `keepster ui` and `keepster ui token rotate` against a real daemon (ticket 60): the real binary, over
//! the control channel, opening a session the loopback listener accepts.

use keepster_testkit::Harness;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn keepster_ui_hands_out_an_address_that_opens_a_browser_session() {
    let daemon = Harness::start().await;

    let run = daemon.run_cli(KEEPSTER, &["ui", "--no-browser"]).await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let url = String::from_utf8(run.stdout).expect("text");
    let address = daemon.loopback().expect("listening");
    let target = url
        .trim()
        .strip_prefix(&format!("http://{address}"))
        .unwrap_or_else(|| panic!("{url:?} is the listener's own address"));
    let exchanged = daemon.loopback_get(target, &[]).await;
    assert_eq!(exchanged.status, 303);
    assert!(exchanged.headers.get("set-cookie").is_some());
}

#[tokio::test]
async fn keepster_ui_token_rotate_replaces_the_token() {
    let daemon = Harness::start().await;
    let before = std::fs::read_to_string(daemon.token_path()).expect("a token");

    let run = daemon.run_cli(KEEPSTER, &["ui", "token", "rotate"]).await;

    assert_eq!(run.exit_code, Some(0), "{}", run.stderr);
    let after = std::fs::read_to_string(daemon.token_path()).expect("a token");
    assert_ne!(before, after);
}

#[tokio::test]
async fn with_the_listener_off_both_commands_fail_alike_and_name_the_key() {
    let off = Harness::start_with_global_config(b"[ui]\nlistener = false\n").await;

    let opened = off.run_cli(KEEPSTER, &["ui", "--no-browser"]).await;
    let rotated = off.run_cli(KEEPSTER, &["ui", "token", "rotate"]).await;

    assert_eq!(
        opened.exit_code,
        Some(5),
        "a state the daemon is in: {}",
        opened.stderr
    );
    assert_eq!(rotated.exit_code, opened.exit_code);
    for run in [&opened, &rotated] {
        assert!(run.stdout.is_empty());
        assert!(run.stderr.contains("ui.listener"), "{}", run.stderr);
        assert!(
            run.stderr
                .contains(&off.config_path().display().to_string()),
            "the file to switch it back on in: {}",
            run.stderr
        );
        assert!(
            run.stderr.contains("listener_unavailable"),
            "{}",
            run.stderr
        );
    }
}

#[tokio::test]
async fn status_says_whether_there_is_a_network_surface_and_its_absence_is_no_fault() {
    let listening = Harness::start().await;
    let off = Harness::start_with_global_config(b"[ui]\nlistener = false\n").await;

    let shown = listening.run_cli(KEEPSTER, &["status"]).await;
    let declined = off.run_cli(KEEPSTER, &["status"]).await;

    let address = listening.loopback().expect("listening").to_string();
    let text = String::from_utf8(shown.stdout).expect("text");
    assert!(
        text.contains(&format!("listening on http://{address}")),
        "{text}"
    );
    let text = String::from_utf8(declined.stdout).expect("text");
    assert!(text.contains("off (ui.listener = false)"), "{text}");
    assert_eq!(
        declined.exit_code, shown.exit_code,
        "declining the surface changes no exit code"
    );
    assert_eq!(declined.exit_code, Some(0), "{}", declined.stderr);
}
