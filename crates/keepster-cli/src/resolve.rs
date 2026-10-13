//! Working out which Root a command is about: a Watched Root, or — for the commands that act on a
//! history rather than on a directory — one no longer watched, by its full identity.
//!
//! All of this is client-side by design (ticket 07): the API takes one canonical identity in a URL, and
//! the conveniences — a working directory, an id prefix, a path — are resolved here, so ambiguity is
//! settled by the person in the terminal and never leaks into a URL.

use keepster_api::{Root, UnwatchedRoot, encode_path_query};

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

/// A Root a command acts on: its identity, and how a message names it — its path, or for a Root no
/// longer watched whose record says nothing, its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    /// The canonical identity the API addresses it by.
    pub(crate) id: String,
    /// How a message names it.
    pub(crate) named: String,
}

impl From<Root> for Target {
    fn from(root: Root) -> Self {
        Self {
            id: root.id,
            named: root.path,
        }
    }
}

/// The Roots a command that takes `--root` or `--all` acts on. A command that erases history reaches
/// Roots no longer watched too: by their identity, or all of them with `--all` (ticket 68).
pub(crate) async fn targets(
    client: &DaemonClient,
    selector: Option<&str>,
    all_of_them: bool,
    erasing: bool,
) -> Result<Vec<Target>, u8> {
    if all_of_them {
        all(client, erasing).await
    } else if erasing {
        Ok(vec![held(client, selector).await?])
    } else {
        Ok(vec![resolve(client, selector).await?.into()])
    }
}

/// The Root a command acting on a history is about.
///
/// A full identity is taken as it is when no watched Root has it: a Root no longer watched keeps its
/// history, its last state pinned, and its identity is all that is left to reach it by (ticket 53) —
/// to pin in it, or to erase it (ticket 68). Whether the daemon still holds it is the daemon's answer to
/// the request that follows.
pub(crate) async fn held(client: &DaemonClient, selector: Option<&str>) -> Result<Target, u8> {
    let identity = selector.filter(|selector| crate::selector::is_identity(selector));
    if let Some(identity) = identity {
        let watched = client
            .get(&format!("/v1/roots/{identity}"))
            .await
            .ok()
            .filter(|response| response.status.is_success())
            .and_then(|response| serde_json::from_slice::<Root>(&response.body).ok());

        return Ok(match watched {
            Some(root) => root.into(),
            None => Target {
                id: identity.to_ascii_uppercase(),
                named: format!("the Root {identity}"),
            },
        });
    }

    Ok(resolve(client, selector).await?.into())
}

/// Every Root an all-Roots command acts on: every watched Root, then — for a command that erases
/// history — every Root no longer watched, whose history is reached as [`held`] reaches one.
async fn all(client: &DaemonClient, with_unwatched: bool) -> Result<Vec<Target>, u8> {
    let watched: Vec<Root> = answered(client.get("/v1/roots").await)?;
    let mut every: Vec<Target> = watched.into_iter().map(Target::from).collect();
    if with_unwatched {
        let unwatched: Vec<UnwatchedRoot> = answered(client.get("/v1/unwatched-roots").await)?;
        every.extend(unwatched.into_iter().map(|root| Target {
            named: root.path.unwrap_or_else(|| format!("the Root {}", root.id)),
            id: root.id,
        }));
    }

    Ok(every)
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

pub(crate) async fn by_path(client: &DaemonClient, path: &str) -> Result<Root, u8> {
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
