//! The embedded web UI (ticket 64), served on the loopback listener behind the same gate as `/v1`:
//! Content-Types, caching, the single-page fallback, and the headers that keep the page from making any
//! request outside the daemon.

use keepster_cli::Response;
use keepster_testkit::Harness;

/// A browser session: the cookie a bootstrap code is exchanged for.
async fn session(daemon: &Harness) -> String {
    let answer = daemon
        .post_json("/v1/ui/bootstrap", &serde_json::json!({}))
        .await;
    let url = answer["url"].as_str().expect("a URL");
    let target = &url[url.find("/?bootstrap=").expect("a bootstrap URL")..];
    let exchanged = daemon.loopback_get(target, &[]).await;
    let cookie = header(&exchanged, "set-cookie").expect("a session cookie");

    cookie
        .split(';')
        .next()
        .expect("a name and a value")
        .to_owned()
}

/// A response header's value, when it is present and text.
fn header<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .get(name)
        .and_then(|value| value.to_str().ok())
}

/// The first `/assets/…` path the page names in quotes that ends in `extension`.
fn named_asset(page: &Response, extension: &str) -> String {
    let html = String::from_utf8_lossy(&page.body);
    html.match_indices("\"/assets/")
        .map(|(at, _)| {
            let rest = &html[at + 1..];
            rest[..rest.find('"').expect("a closing quote")].to_owned()
        })
        .find(|path| path.ends_with(extension))
        .unwrap_or_else(|| panic!("the page names no {extension} asset: {html}"))
}

#[tokio::test]
async fn the_app_page_is_served_at_the_root_and_revalidated_every_time() {
    let daemon = Harness::start().await;
    let cookie = session(&daemon).await;

    let page = daemon.loopback_get("/", &[("cookie", &cookie)]).await;

    assert_eq!(page.status, 200);
    assert_eq!(header(&page, "content-type"), Some("text/html"));
    assert_eq!(header(&page, "cache-control"), Some("no-cache"));
    assert!(header(&page, "etag").is_some());
    assert!(String::from_utf8_lossy(&page.body).contains("<div id=\"app\">"));
}

#[tokio::test]
async fn the_page_may_load_nothing_from_outside_the_daemon() {
    let daemon = Harness::start().await;
    let cookie = session(&daemon).await;

    let page = daemon.loopback_get("/", &[("cookie", &cookie)]).await;

    assert_eq!(
        header(&page, "content-security-policy"),
        Some(
            "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; \
             base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
        )
    );
    assert_eq!(header(&page, "x-content-type-options"), Some("nosniff"));
}

#[tokio::test]
async fn the_assets_the_page_names_are_served_with_their_types_and_cached_for_good() {
    let daemon = Harness::start().await;
    let cookie = session(&daemon).await;
    let page = daemon.loopback_get("/", &[("cookie", &cookie)]).await;

    for (extension, content_type) in [(".js", "text/javascript"), (".css", "text/css")] {
        let path = named_asset(&page, extension);
        let asset = daemon.loopback_get(&path, &[("cookie", &cookie)]).await;

        assert_eq!(asset.status, 200, "{path}");
        assert_eq!(header(&asset, "content-type"), Some(content_type), "{path}");
        assert_eq!(
            header(&asset, "cache-control"),
            Some("public, max-age=31536000, immutable"),
            "{path}"
        );
        assert_eq!(header(&asset, "x-content-type-options"), Some("nosniff"));
    }
}

#[tokio::test]
async fn a_copy_the_browser_holds_is_not_sent_again() {
    let daemon = Harness::start().await;
    let cookie = session(&daemon).await;
    let page = daemon.loopback_get("/", &[("cookie", &cookie)]).await;
    let etag = header(&page, "etag").expect("a tag");

    let again = daemon
        .loopback_get("/", &[("cookie", &cookie), ("if-none-match", etag)])
        .await;

    assert_eq!(again.status, 304);
    assert_eq!(header(&again, "etag"), Some(etag));
    assert_eq!(
        header(&again, "cache-control"),
        Some("no-cache"),
        "a 304 carries the caching a 200 would have (RFC 9110 §15.4.5)"
    );
    assert!(again.body.is_empty());
}

#[tokio::test]
async fn a_page_of_the_app_is_answered_with_the_app() {
    let daemon = Harness::start().await;
    let cookie = session(&daemon).await;
    let root = daemon.loopback_get("/", &[("cookie", &cookie)]).await;

    let page = daemon
        .loopback_get("/history/somewhere", &[("cookie", &cookie)])
        .await;

    assert_eq!(page.status, 200);
    assert_eq!(header(&page, "content-type"), Some("text/html"));
    assert_eq!(page.body, root.body);
}

#[tokio::test]
async fn a_missing_file_and_an_unknown_api_path_are_not_answered_with_the_page() {
    let daemon = Harness::start().await;
    let cookie = session(&daemon).await;

    for path in ["/assets/missing.js", "/v1/nothing-here", "/v1"] {
        let missing = daemon.loopback_get(path, &[("cookie", &cookie)]).await;

        assert_eq!(missing.status, 404, "{path}");
        assert_ne!(
            header(&missing, "content-type"),
            Some("text/html"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn without_a_session_nothing_of_the_app_is_served() {
    let daemon = Harness::start().await;
    let cookie = session(&daemon).await;
    let page = daemon.loopback_get("/", &[("cookie", &cookie)]).await;
    let script = named_asset(&page, ".js");

    for path in ["/", script.as_str(), "/history"] {
        let refused = daemon.loopback_get(path, &[]).await;

        assert_eq!(refused.status, 401, "{path}");
    }
}

#[tokio::test]
async fn the_app_is_only_read() {
    let daemon = Harness::start().await;
    let cookie = session(&daemon).await;
    let host = daemon.loopback().expect("listening").to_string();

    let posted = daemon
        .loopback_request("POST", "/", &[("host", &host), ("cookie", &cookie)])
        .await;

    assert_eq!(posted.status, 405);
    assert_eq!(header(&posted, "allow"), Some("GET, HEAD"));
}
