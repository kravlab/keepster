//! Working out which Watched Root a command is about.
//!
//! All of this is client-side by design (ticket 07): the API takes one canonical identity in a URL, and
//! the conveniences — a working directory, an id prefix, a path — are resolved here, so ambiguity is
//! settled by the person in the terminal and never leaks into a URL.

use keepster_api::{Root, encode_path_query};

use crate::client::DaemonClient;
use crate::exit;
use crate::roots::answered;

/// Find the Root a command should act on.
///
/// With no selector the working directory decides, which is the common case — a person standing in
/// their project does not want to name it. A selector may be a full identity, an unambiguous prefix of
/// one, or a path, and each is tried in that order at the cost of at most one request.
pub async fn resolve(client: &DaemonClient, selector: Option<&str>) -> Result<Root, u8> {
    match selector {
        Some(selector) => by_selector(client, selector).await,
        None => {
            let directory = std::env::current_dir().map_err(|error| {
                eprintln!("keepster: the working directory could not be read: {error}");
                exit::OTHER
            })?;

            by_path(client, &directory.to_string_lossy()).await
        }
    }
}

async fn by_selector(client: &DaemonClient, selector: &str) -> Result<Root, u8> {
    // A full identity is exactly what the Root resource is addressed by, so asking for it settles the
    // common case without fetching the Registry at all. Anything else — a prefix, a path, a typo — comes
    // back as a refusal, which is not an error here but the next thing to try.
    if let Ok(response) = client.get(&format!("/v1/roots/{selector}")).await {
        if response.status.is_success() {
            if let Ok(root) = serde_json::from_slice::<Root>(&response.body) {
                return Ok(root);
            }
        }
    }

    let registered: Vec<Root> = answered(client.get("/v1/roots").await)?;
    let matching: Vec<&Root> = registered
        .iter()
        .filter(|root| root.id.starts_with(selector))
        .collect();

    match matching.as_slice() {
        [root] => Ok((*root).clone()),
        [] => by_path(client, selector).await,
        several => {
            // A prefix that fits several Roots is a question only the person can answer, so it is a
            // usage error rather than a guess.
            eprintln!("keepster: `{selector}` matches {} Roots:", several.len());
            for root in several {
                eprintln!("  {}  {}", root.id, root.path);
            }

            Err(exit::USAGE)
        }
    }
}

async fn by_path(client: &DaemonClient, path: &str) -> Result<Root, u8> {
    let absolute = std::path::absolute(path).map_err(|error| {
        eprintln!("keepster: {path} could not be resolved: {error}");
        exit::OTHER
    })?;

    let found: Vec<Root> = answered(
        client
            .get(&format!(
                "/v1/roots?path={}",
                encode_path_query(&absolute.to_string_lossy())
            ))
            .await,
    )?;

    match found.into_iter().next() {
        Some(root) => Ok(root),
        None => {
            eprintln!(
                "keepster: {} is not inside a Watched Root; run `keepster watch {}` to start one",
                absolute.display(),
                absolute.display()
            );

            Err(exit::NOT_FOUND)
        }
    }
}
