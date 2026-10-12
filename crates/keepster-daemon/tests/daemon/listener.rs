//! The loopback listener and the browser's way in (ticket 60): a token per start in a `0600` file,
//! `Host` and `Origin` checked on every request, a single-use bootstrap code exchanged for a cookie, and
//! with the listener switched off no network surface and no token at all.

use keepster_testkit::Harness;
use serde_json::Value;

/// The token the daemon wrote for this start.
fn token(daemon: &Harness) -> String {
    std::fs::read_to_string(daemon.token_path()).expect("the token file")
}

fn code_of(response: &keepster_cli::Response) -> Value {
    let problem: Value = serde_json::from_slice(&response.body).unwrap_or_else(|error| {
        panic!(
            "{} is not a problem document ({error}): {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        )
    });
    problem["code"].clone()
}

#[tokio::test]
async fn the_listener_is_on_by_default_and_status_says_where() {
    let daemon = Harness::start().await;

    let status = daemon.get_json("/v1/status").await;

    let address = daemon.loopback().expect("the listener is on by default");
    assert!(address.ip().is_loopback(), "{address}");
    assert_eq!(status["daemon"]["listener"]["state"], "listening");
    assert_eq!(
        status["daemon"]["listener"]["address"],
        address.to_string().as_str()
    );
}

#[tokio::test]
async fn a_token_is_minted_per_start_into_a_file_only_its_owner_can_read() {
    let daemon = Harness::start().await;

    let first = token(&daemon);
    assert_eq!(first.len(), 64, "256 bits, written as hex: {first:?}");
    assert!(
        first.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{first:?}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(daemon.token_path())
            .expect("the token file")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let daemon = daemon.restart().await;

    assert_ne!(token(&daemon), first, "a restart rotates the token");
}

#[tokio::test]
async fn the_version_is_served_before_authentication_and_nothing_else_is() {
    let daemon = Harness::start().await;

    let version = daemon.loopback_get("/v1/version", &[]).await;
    assert_eq!(version.status, 200);

    let status = daemon.loopback_get("/v1/status", &[]).await;
    assert_eq!(status.status, 401);
    assert_eq!(code_of(&status), "unauthorized");

    let wrong = daemon
        .loopback_get("/v1/status", &[("authorization", "Bearer 00")])
        .await;
    assert_eq!(wrong.status, 401);
}

#[tokio::test]
async fn a_client_holding_the_token_is_answered() {
    // A third-party client reads the `0600` file, which is what makes it the machine owner's client.
    let daemon = Harness::start().await;
    let bearer = format!("Bearer {}", token(&daemon));

    let status = daemon
        .loopback_get("/v1/status", &[("authorization", &bearer)])
        .await;

    assert_eq!(
        status.status,
        200,
        "{}",
        String::from_utf8_lossy(&status.body)
    );
}

#[tokio::test]
async fn a_host_other_than_the_loopback_with_its_port_is_refused_even_with_the_token() {
    // DNS rebinding: a page whose own name now resolves to 127.0.0.1 sends its own name as the host.
    let daemon = Harness::start().await;
    let port = daemon.loopback().expect("listening").port();
    let bearer = format!("Bearer {}", token(&daemon));

    for host in [
        "attacker.example".to_owned(),
        format!("attacker.example:{port}"),
        "127.0.0.1".to_owned(),
        format!("127.0.0.1:{}", port.wrapping_add(1)),
    ] {
        let refused = daemon
            .loopback_request(
                "GET",
                "/v1/version",
                &[("host", &host), ("authorization", &bearer)],
            )
            .await;
        assert_eq!(refused.status, 403, "{host}");
        assert_eq!(code_of(&refused), "host_refused", "{host}");
    }

    let named = format!("localhost:{port}");
    let answered = daemon
        .loopback_request(
            "GET",
            "/v1/status",
            &[("host", &named), ("authorization", &bearer)],
        )
        .await;
    assert_eq!(answered.status, 200, "`localhost` names the loopback too");
}

#[tokio::test]
async fn an_origin_not_in_the_allowed_list_is_refused() {
    let daemon = Harness::start().await;
    let address = daemon.loopback().expect("listening");
    let bearer = format!("Bearer {}", token(&daemon));

    let refused = daemon
        .loopback_get(
            "/v1/status",
            &[
                ("origin", "http://attacker.example"),
                ("authorization", &bearer),
            ],
        )
        .await;
    assert_eq!(refused.status, 403);
    assert_eq!(code_of(&refused), "origin_refused");

    let own = format!("http://{address}");
    let answered = daemon
        .loopback_get(
            "/v1/status",
            &[("origin", &own), ("authorization", &bearer)],
        )
        .await;
    assert_eq!(
        answered.status, 200,
        "the listener's own origin is always allowed"
    );
}

#[tokio::test]
async fn a_configured_origin_is_allowed_and_told_so_by_name_never_by_wildcard() {
    let daemon =
        Harness::start_with_global_config(b"[ui]\nallowed_origins = [\"http://localhost:3000\"]\n")
            .await;
    let bearer = format!("Bearer {}", token(&daemon));

    let answered = daemon
        .loopback_get(
            "/v1/status",
            &[
                ("origin", "http://localhost:3000"),
                ("authorization", &bearer),
            ],
        )
        .await;

    assert_eq!(answered.status, 200);
    assert_eq!(
        answered
            .headers
            .get("access-control-allow-origin")
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:3000")
    );
}

#[tokio::test]
async fn with_the_listener_off_there_is_no_network_surface_and_no_token() {
    let daemon = Harness::start().await;
    let stale = daemon.token_path();
    assert!(stale.exists(), "the previous start wrote one");
    daemon.write_global_config(b"[ui]\nlistener = false\n");

    let daemon = daemon.restart().await;

    assert_eq!(daemon.loopback(), None);
    assert!(
        !daemon.token_path().exists(),
        "a secret guarding nothing is not left behind"
    );
    let status = daemon.get_json("/v1/status").await;
    assert_eq!(status["daemon"]["listener"]["state"], "off");
    assert!(
        status["daemon"]["listener"].get("address").is_none(),
        "{status}"
    );
}

/// Ask the control channel for a way into the browser session, and return its URL's path and query.
async fn bootstrap(daemon: &Harness) -> String {
    let answer = daemon
        .post_json("/v1/ui/bootstrap", &serde_json::json!({}))
        .await;
    let url = answer["url"].as_str().expect("a URL");
    let address = daemon.loopback().expect("listening");
    let target = url
        .strip_prefix(&format!("http://{address}"))
        .unwrap_or_else(|| panic!("{url} is the listener's own address"));
    assert!(target.starts_with("/?bootstrap="), "{url}");

    target.to_owned()
}

/// The `name=value` the daemon set as its session cookie, and every attribute it gave it.
fn set_cookie(response: &keepster_cli::Response) -> (String, Vec<String>) {
    let header = response
        .headers
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("a session cookie");
    let mut parts = header.split(';').map(|part| part.trim().to_owned());
    let pair = parts.next().expect("a name and a value");

    (pair, parts.collect())
}

#[tokio::test]
async fn a_bootstrap_code_is_exchanged_once_for_a_cookie_and_redirected_away_from() {
    let daemon = Harness::start().await;
    let target = bootstrap(&daemon).await;

    let exchanged = daemon.loopback_get(&target, &[]).await;

    assert_eq!(exchanged.status, 303);
    assert_eq!(
        exchanged
            .headers
            .get("location")
            .and_then(|value| value.to_str().ok()),
        Some("/"),
        "a clean URL, so the code leaves the address bar"
    );
    let (cookie, attributes) = set_cookie(&exchanged);
    assert!(
        attributes.iter().any(|attribute| attribute == "HttpOnly"),
        "{attributes:?}"
    );
    assert!(
        attributes
            .iter()
            .any(|attribute| attribute == "SameSite=Strict"),
        "{attributes:?}"
    );
    assert!(
        !cookie.contains(target.trim_start_matches("/?bootstrap=")),
        "the cookie is not the code"
    );

    let landed = daemon.loopback_get("/", &[("cookie", &cookie)]).await;
    assert_eq!(landed.status, 200);
    let status = daemon
        .loopback_get("/v1/status", &[("cookie", &cookie)])
        .await;
    assert_eq!(
        status.status,
        200,
        "{}",
        String::from_utf8_lossy(&status.body)
    );

    let again = daemon.loopback_get(&target, &[]).await;
    assert_eq!(again.status, 401, "the code was burnt");
    assert_eq!(code_of(&again), "unauthorized");
    assert!(again.headers.get("set-cookie").is_none());
}

#[tokio::test]
async fn a_code_the_daemon_never_issued_opens_nothing() {
    let daemon = Harness::start().await;
    let token = token(&daemon);

    for code in ["guess", token.as_str()] {
        let refused = daemon
            .loopback_get(&format!("/?bootstrap={code}"), &[])
            .await;
        assert_eq!(refused.status, 401, "{code}");
        assert!(refused.headers.get("set-cookie").is_none(), "{code}");
    }
}

#[tokio::test]
async fn the_browser_way_in_is_handed_out_on_the_control_channel_alone() {
    let daemon = Harness::start().await;
    let bearer = format!("Bearer {}", token(&daemon));
    let host = daemon.loopback().expect("listening").to_string();

    for path in ["/v1/ui/bootstrap", "/v1/ui/token/rotate"] {
        let answered = daemon
            .loopback_request("POST", path, &[("host", &host), ("authorization", &bearer)])
            .await;
        assert_eq!(answered.status, 404, "{path}");
    }
}

#[tokio::test]
async fn rotating_revokes_every_session_and_every_code_not_yet_exchanged() {
    let daemon = Harness::start().await;
    let first = token(&daemon);
    let exchanged = daemon.loopback_get(&bootstrap(&daemon).await, &[]).await;
    let (cookie, _) = set_cookie(&exchanged);
    let pending = bootstrap(&daemon).await;

    let rotated = daemon
        .post("/v1/ui/token/rotate", &serde_json::json!({}))
        .await;

    assert_eq!(rotated.status, 204);
    let second = token(&daemon);
    assert_ne!(second, first, "a new token is in its file");
    let revoked = daemon
        .loopback_get("/v1/status", &[("cookie", &cookie)])
        .await;
    assert_eq!(
        revoked.status, 401,
        "the session the old token opened is over"
    );
    assert_eq!(daemon.loopback_get(&pending, &[]).await.status, 401);
    let bearer = format!("Bearer {second}");
    let answered = daemon
        .loopback_get("/v1/status", &[("authorization", &bearer)])
        .await;
    assert_eq!(answered.status, 200, "the new token is in force");
}

#[tokio::test]
async fn a_restart_ends_every_browser_session() {
    let daemon = Harness::start().await;
    let exchanged = daemon.loopback_get(&bootstrap(&daemon).await, &[]).await;
    let (cookie, _) = set_cookie(&exchanged);

    let daemon = daemon.restart().await;

    let refused = daemon
        .loopback_get("/v1/status", &[("cookie", &cookie)])
        .await;
    assert_eq!(refused.status, 401);
}

#[tokio::test]
async fn with_the_listener_off_there_is_no_way_in_and_no_token_to_rotate() {
    let daemon = Harness::start_with_global_config(b"[ui]\nlistener = false\n").await;

    let opened = daemon
        .post("/v1/ui/bootstrap", &serde_json::json!({}))
        .await;
    let rotated = daemon
        .post("/v1/ui/token/rotate", &serde_json::json!({}))
        .await;

    for refused in [&opened, &rotated] {
        assert_eq!(refused.status, 409);
        assert_eq!(code_of(refused), "listener_unavailable");
        let problem: Value = serde_json::from_slice(&refused.body).expect("a problem");
        assert_eq!(problem["details"]["reason"], "off");
    }
    assert!(!daemon.token_path().exists(), "rotating did not mint one");
}

#[tokio::test]
async fn declaring_a_cli_only_install_creates_a_configuration_without_the_listener() {
    let daemon = Harness::start().await;
    assert!(!daemon.config_path().exists());

    let declared = keepster_service::declare_no_web_ui(&daemon.config_path()).expect("declared");

    assert_eq!(declared, keepster_service::NoWebUi::Created);
    let daemon = daemon.restart().await;
    assert_eq!(daemon.loopback(), None);
    let status = daemon.get_json("/v1/status").await;
    assert_eq!(status["daemon"]["listener"]["state"], "off");
}

#[tokio::test]
async fn declaring_a_cli_only_install_never_rewrites_an_existing_configuration() {
    // Editing a person's TOML means keeping their comments and their order; it is theirs to edit, and
    // they are told the line to add.
    let daemon = Harness::start().await;
    let written = b"# mine\n[files]\nexclude = [\"*.log\"]\n";
    daemon.write_global_config(written);

    let declared = keepster_service::declare_no_web_ui(&daemon.config_path()).expect("declared");

    let keepster_service::NoWebUi::Existing { add } = declared else {
        panic!("an existing file is left alone: {declared:?}");
    };
    assert!(add.contains("listener = false"), "{add}");
    assert_eq!(
        std::fs::read(daemon.config_path()).expect("the file"),
        written
    );
    let daemon = daemon.restart().await;
    assert!(daemon.loopback().is_some(), "nothing changed");
}
