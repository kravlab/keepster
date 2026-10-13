//! Embeds the pre-built SPA (`ui/dist`) and decides how each of its files is served: the Content-Type,
//! the caching, and the single-page fallback (ticket 08). It knows no HTTP stack: the daemon turns an
//! [`Answer`] into a response, behind the loopback listener's gate.

use std::borrow::Cow;

use rust_embed::Embed;

/// `ui/dist` (the folder is relative to this crate), built by `make ui` and committed, so `cargo build`
/// never needs node (tickets 08, 27).
#[derive(Embed)]
#[folder = "../../ui/dist"]
#[exclude = ".vite/*"]
struct Dist;

/// What a request for `path` is answered with.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer {
    /// The file, to send with these headers.
    File(File),
    /// The client's copy, named by `If-None-Match`, is current: `304` with this `ETag`, and the caching the
    /// `200` would have carried, as RFC 9110 §15.4.5 asks.
    NotModified {
        /// The tag that matched.
        etag: String,
        /// How long a browser may keep it.
        cache_control: &'static str,
    },
    /// No such file, and the path does not name a page of the app.
    NotFound,
}

/// One embedded file, ready to send.
#[derive(Debug, PartialEq, Eq)]
pub struct File {
    /// Its Content-Type, from its extension.
    pub content_type: String,
    /// How long a browser may keep it.
    pub cache_control: &'static str,
    /// Names its content: a hash of the bytes.
    pub etag: String,
    /// Its bytes.
    pub body: Cow<'static, [u8]>,
}

/// The app's page: what `/` is, and what every other page of the app is too.
const PAGE: &str = "index.html";

/// Vite's output, named by a hash of its content: a new build is a new name, so a copy never goes stale.
const HASHED: &str = "assets/";

/// For a file whose name never changes with its content: kept, but checked with the daemon before use, so
/// a new build is picked up at once.
const REVALIDATE: &str = "no-cache";

/// For a file whose name is a hash of its content: kept for a year without asking.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// Answers a request for the URL path `path`, given the request's `If-None-Match`.
///
/// A path that names no file but has no extension in its last segment is a page of the app, which the app
/// routes itself, so it is answered with the page. A missing path with an extension stays missing: a
/// script or stylesheet answered with HTML fails in a way that hides the real error.
#[must_use]
pub fn answer(path: &str, if_none_match: Option<&str>) -> Answer {
    let name = match path.trim_start_matches('/') {
        "" => PAGE,
        name => name,
    };
    let (name, embedded) = match Dist::get(name) {
        Some(embedded) => (name, embedded),
        None if !last_segment(name).contains('.') => match Dist::get(PAGE) {
            Some(embedded) => (PAGE, embedded),
            None => return Answer::NotFound,
        },
        None => return Answer::NotFound,
    };

    let etag = format!("\"{}\"", hex(&embedded.metadata.sha256_hash()));
    let cache_control = if name.starts_with(HASHED) {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    if if_none_match.is_some_and(|tags| names_tag(tags, &etag)) {
        return Answer::NotModified {
            etag,
            cache_control,
        };
    }

    Answer::File(File {
        content_type: embedded.metadata.mimetype().to_owned(),
        cache_control,
        etag,
        body: embedded.data,
    })
}

/// The part of a path after its last `/`.
fn last_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether an `If-None-Match` list names `etag`, compared weakly as RFC 9110 asks for a `GET`.
fn names_tag(tags: &str, etag: &str) -> bool {
    tags.split(',')
        .map(|tag| tag.trim().trim_start_matches("W/"))
        .any(|tag| tag == "*" || tag == etag)
}

/// Lower-case hex of `bytes`.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded files under `assets/`, Vite's hashed output.
    fn assets() -> Vec<String> {
        Dist::iter()
            .filter(|name| name.starts_with("assets/"))
            .map(Cow::into_owned)
            .collect()
    }

    /// The file an answer carries; anything else fails the test.
    fn file(answer: Answer) -> File {
        match answer {
            Answer::File(file) => file,
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn the_root_is_the_app_page_and_is_revalidated_every_time() {
        let page = file(answer("/", None));

        assert_eq!(page.content_type, "text/html");
        assert_eq!(page.cache_control, "no-cache");
        assert!(String::from_utf8_lossy(&page.body).contains("<div id=\"app\">"));
    }

    #[test]
    fn a_hashed_asset_is_cached_for_good() {
        let name = assets()
            .into_iter()
            .find(|name| name.ends_with(".js"))
            .expect("a script");

        let script = file(answer(&format!("/{name}"), None));

        assert_eq!(script.content_type, "text/javascript");
        assert_eq!(script.cache_control, "public, max-age=31536000, immutable");
    }

    #[test]
    fn a_stylesheet_says_so() {
        let name = assets()
            .into_iter()
            .find(|name| name.ends_with(".css"))
            .expect("a stylesheet");

        assert_eq!(
            file(answer(&format!("/{name}"), None)).content_type,
            "text/css"
        );
    }

    #[test]
    fn a_path_without_an_extension_is_a_page_of_the_app() {
        assert_eq!(
            file(answer("/history/some/where", None)),
            file(answer("/", None))
        );
    }

    #[test]
    fn a_missing_file_is_not_answered_with_the_page() {
        assert_eq!(answer("/assets/missing.js", None), Answer::NotFound);
        assert_eq!(answer("/favicon.ico", None), Answer::NotFound);
    }

    #[test]
    fn the_build_manifest_is_not_served() {
        assert_eq!(answer("/.vite/manifest.json", None), Answer::NotFound);
    }

    #[test]
    fn a_current_copy_is_not_sent_again() {
        let etag = file(answer("/", None)).etag;

        let current = Answer::NotModified {
            etag: etag.clone(),
            cache_control: "no-cache",
        };

        assert_eq!(answer("/", Some(&etag)), current);
        assert_eq!(answer("/", Some(&format!("\"stale\", {etag}"))), current);
        assert!(matches!(answer("/", Some("\"stale\"")), Answer::File(_)));
    }

    #[test]
    fn a_tag_is_quoted_and_changes_with_the_content() {
        let page = file(answer("/", None)).etag;
        let name = assets().into_iter().next().expect("an asset");
        let asset = file(answer(&format!("/{name}"), None)).etag;

        assert!(page.starts_with('"') && page.ends_with('"'), "{page}");
        assert_ne!(page, asset);
    }

    /// The page makes no external request (ticket 64). The daemon's Content-Security-Policy enforces it
    /// in the browser; this keeps a URL from entering the bundle unnoticed. The hosts allowed are never
    /// fetched: XML namespace names, and the documentation links in Svelte's error messages.
    #[test]
    fn no_embedded_file_names_an_outside_host() {
        const NEVER_FETCHED: [&str; 2] = ["www.w3.org", "svelte.dev"];
        let mut outside = Vec::new();
        for name in Dist::iter() {
            let bytes = Dist::get(&name).expect("listed").data;
            let text = String::from_utf8_lossy(&bytes);
            for (at, _) in text.match_indices("://") {
                let scheme = text[..at]
                    .rsplit(|c: char| !c.is_ascii_alphabetic())
                    .next()
                    .unwrap_or("");
                if scheme != "http" && scheme != "https" {
                    continue;
                }
                let host: String = text[at + 3..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
                    .collect();
                if !host.is_empty() && !NEVER_FETCHED.contains(&host.as_str()) {
                    outside.push(format!("{name}: {scheme}://{host}"));
                }
            }
        }

        assert!(outside.is_empty(), "{outside:#?}");
    }
}
