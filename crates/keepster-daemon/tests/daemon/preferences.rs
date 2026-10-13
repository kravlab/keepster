//! The web UI's preferences at the `/v1` seam (ticket 66, ticket 16): the defaults until something is
//! chosen, replaced whole by `PUT`, refused when a value is out of range, kept across a restart, and the
//! same on both listeners.

use keepster_testkit::Harness;
use serde_json::{Value, json};

/// The `code` of a problem document.
fn code_of(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice::<Value>(&response.body).expect("a problem")["code"].clone()
}

/// A response's body as JSON.
fn json_of(response: &keepster_cli::Response) -> Value {
    serde_json::from_slice(&response.body).expect("a JSON body")
}

/// The preferences before anything is chosen, as `/v1` spells them.
fn defaults() -> Value {
    json!({ "theme": "system", "syntax_line_limit": 20000, "wrap_lines": false })
}

/// Preferences that differ from the defaults in every key.
fn chosen() -> Value {
    json!({ "theme": "dark", "syntax_line_limit": 5000, "wrap_lines": true })
}

#[tokio::test]
async fn nothing_chosen_reads_as_the_defaults() {
    let daemon = Harness::start().await;

    assert_eq!(daemon.get_json("/v1/preferences").await, defaults());
}

#[tokio::test]
async fn what_is_put_is_answered_and_read_back() {
    let daemon = Harness::start().await;

    let put = daemon.put_json("/v1/preferences", &chosen()).await;

    assert_eq!(put.status, 200, "{}", String::from_utf8_lossy(&put.body));
    assert_eq!(json_of(&put), chosen(), "the answer is what is now stored");
    assert_eq!(daemon.get_json("/v1/preferences").await, chosen());
}

#[tokio::test]
async fn a_value_out_of_range_is_refused_and_nothing_changes() {
    let daemon = Harness::start().await;
    daemon.put_json("/v1/preferences", &chosen()).await;

    for body in [
        json!({ "theme": "blue", "syntax_line_limit": 5000, "wrap_lines": true }),
        json!({ "theme": "light", "syntax_line_limit": 0, "wrap_lines": true }),
        json!({ "theme": "light", "syntax_line_limit": -1, "wrap_lines": true }),
        json!({ "theme": "light", "syntax_line_limit": 10_000_001, "wrap_lines": true }),
    ] {
        let refused = daemon.put_json("/v1/preferences", &body).await;
        assert_eq!(refused.status, 400, "{body}");
        assert_eq!(code_of(&refused), "preference_invalid", "{body}");
    }
    assert_eq!(daemon.get_json("/v1/preferences").await, chosen());
}

#[tokio::test]
async fn a_body_missing_a_key_is_not_the_json_the_endpoint_takes() {
    let daemon = Harness::start().await;

    let refused = daemon
        .put_json("/v1/preferences", &json!({ "theme": "dark" }))
        .await;

    assert_eq!(refused.status, 422);
    assert_eq!(code_of(&refused), "body_invalid");
    assert_eq!(daemon.get_json("/v1/preferences").await, defaults());
}

#[tokio::test]
async fn what_is_chosen_survives_a_restart() {
    let daemon = Harness::start().await;
    daemon.put_json("/v1/preferences", &chosen()).await;

    let daemon = daemon.restart().await;

    assert_eq!(daemon.get_json("/v1/preferences").await, chosen());
}

#[tokio::test]
async fn the_loopback_listener_serves_the_same_preferences() {
    let daemon = Harness::start().await;
    daemon.put_json("/v1/preferences", &chosen()).await;
    let token = std::fs::read_to_string(daemon.token_path()).expect("the token file");
    let bearer = format!("Bearer {}", token.trim());

    let read = daemon
        .loopback_get("/v1/preferences", &[("authorization", &bearer)])
        .await;

    assert_eq!(read.status, 200, "{}", String::from_utf8_lossy(&read.body));
    assert_eq!(json_of(&read), chosen());
}

#[tokio::test]
async fn a_file_that_cannot_be_read_is_served_as_the_defaults_and_replaced_by_the_next_put() {
    let daemon = Harness::start().await;
    std::fs::write(
        daemon.store_base().join("preferences.toml"),
        "theme = \"blue\"\n",
    )
    .expect("a file naming no theme there is");

    assert_eq!(daemon.get_json("/v1/preferences").await, defaults());

    let put = daemon.put_json("/v1/preferences", &chosen()).await;
    assert_eq!(put.status, 200, "{}", String::from_utf8_lossy(&put.body));
    assert_eq!(daemon.get_json("/v1/preferences").await, chosen());
}

#[tokio::test]
async fn a_put_that_cannot_be_written_says_so() {
    // A directory where the file goes: nothing can be renamed over it, on any platform.
    let daemon = Harness::start().await;
    std::fs::create_dir(daemon.store_base().join("preferences.toml"))
        .expect("a directory in the way");

    let failed = daemon.put_json("/v1/preferences", &chosen()).await;

    assert_eq!(
        failed.status,
        500,
        "{}",
        String::from_utf8_lossy(&failed.body)
    );
    assert_eq!(code_of(&failed), "preferences_failed");
    assert_eq!(daemon.get_json("/v1/preferences").await, defaults());
}
