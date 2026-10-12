//! The CLI driven against a real daemon over a real socket: "the CLI is a thin client over the same
//! API" (ticket 07) stated as a test rather than as prose.

use keepster_testkit::Harness;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

#[tokio::test]
async fn cli_status_json_prints_the_api_payload_byte_for_byte() {
    let daemon = Harness::start().await;

    // `uptime_seconds` advances once a second, so the payload is only comparable when it did not
    // change around the CLI run. Reading it either side and repeating the *setup* — never the
    // assertion — keeps this deterministic instead of letting a second boundary make it flaky.
    for _ in 0..5 {
        let before = daemon.get("/v1/status").await.body;
        let run = daemon.run_cli(KEEPSTER, &["status", "--json"]).await;
        let after = daemon.get("/v1/status").await.body;

        if before != after {
            continue;
        }

        assert_eq!(
            run.exit_code,
            Some(0),
            "expected success, stderr was: {}",
            run.stderr
        );
        assert_eq!(
            run.stdout, before,
            "--json must print the daemon's payload unchanged, not a re-serialised copy"
        );
        return;
    }

    panic!("the status payload never held still long enough to compare it byte for byte");
}

#[tokio::test]
async fn cli_status_renders_the_daemon_rows_for_a_human() {
    let daemon = Harness::start().await;

    let run = daemon.run_cli(KEEPSTER, &["status"]).await;

    assert_eq!(run.exit_code, Some(0), "stderr was: {}", run.stderr);

    let out = String::from_utf8(run.stdout).expect("human output is text");
    assert!(
        out.contains(env!("CARGO_PKG_VERSION")),
        "no build version in:\n{out}"
    );
    // Every row `status` can determine today is named, including the ones it cannot: ticket 05
    // requires the tool never to look healthier than it is.
    for row in ["api", "pid", "uptime", "autostart", "linger", "maintenance"] {
        assert!(out.contains(row), "no `{row}` row in:\n{out}");
    }
    // Autostart and linger are read from the machine the test runs on (ticket 61): whichever they are,
    // the row says so in words, never as a bare default.
    let row = |name: &str| {
        out.lines()
            .find(|line| line.trim_start().starts_with(name))
            .unwrap_or_else(|| panic!("no `{name}` row in:\n{out}"))
            .to_owned()
    };
    let autostart = row("autostart");
    assert!(
        ["available", "not installed", "disabled", "unknown"]
            .iter()
            .any(|said| autostart.contains(said)),
        "{autostart}"
    );
    let linger = row("linger");
    assert!(
        ["available", "coverage ends at logout", "unknown"]
            .iter()
            .any(|said| linger.contains(said)),
        "{linger}"
    );
    assert!(
        out.to_lowercase().contains("no watched roots"),
        "an empty Registry must be stated, not left blank, in:\n{out}"
    );
}

#[tokio::test]
async fn cli_status_exits_three_when_no_daemon_is_listening() {
    // A dead daemon and a healthy one must be distinguishable by a script, which is why ticket 07
    // makes the exit codes a table rather than `1` for everything.
    let daemon = Harness::start().await;
    let unused = daemon.control_path().with_extension("absent");

    let run = daemon
        .run_cli_with_control_path(KEEPSTER, &unused, &["status"])
        .await;

    assert_eq!(
        run.exit_code,
        Some(3),
        "expected the daemon-unreachable code, stderr was: {}",
        run.stderr
    );
    assert!(
        run.stderr.contains(&unused.display().to_string()),
        "the failure must name the control channel it tried, in:\n{}",
        run.stderr
    );
}
