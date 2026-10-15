//! The `path=` query parameter, read as the bytes it percent-encodes (ticket 74).
//!
//! axum's `Query` decodes through `serde_urlencoded`, which turns a percent-encoded byte that is not
//! UTF-8 into U+FFFD before a handler sees it — and then two names that differ only there are one. A
//! path is bytes, so `path=` is read from the raw query string itself; every other parameter still goes
//! through `Query`, and the parameter structs leave `path` out so it is read once, here.

use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use keepster_api::ErrorCode;
use keepster_core::RootPath;

use crate::problem::ProblemResponse;
use crate::snapshots::parse_path;

/// The `path=` parameter of a request, checked as every endpoint checks a path; `None` when absent.
pub(crate) struct PathParameter(pub(crate) Option<RootPath>);

impl<S: Send + Sync> FromRequestParts<S> for PathParameter {
    type Rejection = ProblemResponse;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let found = raw_path(parts.uri.query().unwrap_or_default())?;

        found.map(|bytes| parse_path(&bytes)).transpose().map(Self)
    }
}

/// The bytes the one `path=` of `query` encodes, or `None` without one. A second is refused, as the
/// typed parameters refuse a repeated one.
fn raw_path(query: &str) -> Result<Option<Vec<u8>>, ProblemResponse> {
    let mut found = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if decoded(key).as_deref() != Some(&b"path"[..]) {
            continue;
        }
        if found.is_some() {
            return Err(ProblemResponse::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::PathInvalid,
                "`path` is given more than once",
            ));
        }
        found = Some(decoded(value).ok_or_else(|| {
            ProblemResponse::new(
                StatusCode::BAD_REQUEST,
                ErrorCode::PathInvalid,
                "`path` holds a `%` that is not followed by two hexadecimal digits",
            )
        })?);
    }

    Ok(found)
}

/// One key or value of a query string as bytes: `+` is a space, as a form and `URLSearchParams` encode
/// one, and each `%XX` is the byte it names. `+` is read before the escapes, so `%2B` stays a `+`.
///
/// `None` for a `%` that escapes no byte, which a lenient decoder would keep as it stands: such a value
/// names no path any client meant, so `path=` refuses it rather than guess.
fn decoded(text: &str) -> Option<Vec<u8>> {
    let escapes_bytes = text.split('%').skip(1).all(|after| {
        after
            .as_bytes()
            .get(..2)
            .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
    });

    escapes_bytes.then(|| percent_encoding::percent_decode_str(&text.replace('+', " ")).collect())
}

#[cfg(test)]
mod tests {
    use super::raw_path;

    #[test]
    fn every_byte_is_read_and_nothing_else_is_taken_for_the_path() {
        assert_eq!(
            raw_path("depth=1&path=src/%FF%20a+b%2Bc").ok().flatten(),
            Some(b"src/\xff a b+c".to_vec())
        );
        assert_eq!(raw_path("depth=1&paths=x").ok(), Some(None));
        assert_eq!(raw_path("").ok(), Some(None));
        assert_eq!(
            raw_path("path").ok().flatten(),
            Some(Vec::new()),
            "the Root itself"
        );
    }

    #[test]
    fn a_repeated_path_is_refused() {
        assert!(raw_path("path=a&path=b").is_err());
    }

    #[test]
    fn a_percent_sign_that_escapes_no_byte_is_refused() {
        assert!(raw_path("path=%G1").is_err());
        assert!(raw_path("path=a%F").is_err());
        assert!(raw_path("path=%").is_err());
        assert_eq!(
            raw_path("depth=%G1&path=a").ok().flatten(),
            Some(b"a".to_vec()),
            "another parameter is not this one's to refuse"
        );
    }
}
