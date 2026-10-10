//! Serving one version's bytes: whole, by range, or not at all when the client already has them.
//!
//! The bytes are streamed from a blocking task through a bounded channel, so a large version is never
//! held in the response whole, and a ranged read hands on only the range (ticket 07: a UI showing the
//! head of a 200 MB file must not fetch all of it).

use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use keepster_app::snapshots::{self, StoreLocation};
use keepster_core::{RootPath, SnapshotId};
use keepster_store::StoreError;
use tokio::sync::{mpsc, oneshot};

use crate::problem::ProblemResponse;
use crate::snapshots::store_problem;

/// How many chunks may wait in the channel before the reader is made to wait for the client.
///
/// Small on purpose: this is what bounds the memory one slow client can pin, not a throughput knob.
const IN_FLIGHT: usize = 4;

/// What a request's headers ask of a version of a known size and tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plan {
    /// The client's copy is current: nothing to send.
    NotModified,
    /// All of it.
    Whole,
    /// `length` bytes from `start`.
    Partial {
        /// The first byte.
        start: u64,
        /// How many bytes.
        length: u64,
    },
    /// A range that lies outside the content.
    Unsatisfiable,
}

/// Decide what to send, from the `Range` and `If-None-Match` headers.
///
/// One range is honoured; several are answered with the whole content, which RFC 9110 permits and
/// which spares every client a multipart body for a case no client of this API needs. A header that
/// cannot be parsed is ignored for the same reason the RFC allows: the whole content is a correct
/// answer to any range request.
pub(crate) fn plan(range: Option<&str>, if_none_match: Option<&str>, size: u64, tag: &str) -> Plan {
    if let Some(candidates) = if_none_match {
        let quoted = format!("\"{tag}\"");
        if candidates.split(',').map(str::trim).any(|candidate| {
            candidate == "*" || candidate == quoted || candidate == format!("W/{quoted}")
        }) {
            return Plan::NotModified;
        }
    }

    let Some(spec) = range.and_then(|range| range.trim().strip_prefix("bytes=")) else {
        return Plan::Whole;
    };
    if spec.contains(',') {
        return Plan::Whole;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return Plan::Whole;
    };

    match (first.trim(), last.trim()) {
        ("", "") => Plan::Whole,
        // The last `n` bytes.
        ("", suffix) => match suffix.parse::<u64>() {
            Ok(0) => Plan::Unsatisfiable,
            Ok(_) if size == 0 => Plan::Unsatisfiable,
            Ok(suffix) => {
                let length = suffix.min(size);
                Plan::Partial {
                    start: size - length,
                    length,
                }
            }
            Err(_) => Plan::Whole,
        },
        (first, last) => {
            let Ok(start) = first.parse::<u64>() else {
                return Plan::Whole;
            };
            let end = match last {
                "" => None,
                last => match last.parse::<u64>() {
                    Ok(end) if end >= start => Some(end),
                    _ => return Plan::Whole,
                },
            };
            if start >= size {
                return Plan::Unsatisfiable;
            }
            let end = end.map_or(size - 1, |end| end.min(size - 1));

            Plan::Partial {
                start,
                length: end - start + 1,
            }
        }
    }
}

/// What the blocking reader learned before the first byte: enough to write the status line and
/// headers.
struct Head {
    size: u64,
    tag: String,
    plan: Plan,
}

/// Serve one version's bytes as the request's headers ask.
pub(crate) async fn serve(
    location: StoreLocation,
    snapshot: SnapshotId,
    path: RootPath,
    headers: &HeaderMap,
) -> Result<Response, ProblemResponse> {
    let range = header_text(headers, header::RANGE);
    let if_none_match = header_text(headers, header::IF_NONE_MATCH);

    let (head_sender, head) = oneshot::channel::<Result<Head, StoreError>>();
    let (chunks, receiver) = mpsc::channel::<Result<Bytes, std::io::Error>>(IN_FLIGHT);

    // The store is neither `Send` nor `Sync`, so it lives and dies on this one blocking thread: it
    // locates the version, reports what it found, and then — only if there is a body to send — streams
    // it into the channel the response reads from.
    tokio::task::spawn_blocking(move || {
        let located = snapshots::open_for_reading(&location).and_then(|store| match store {
            Some(store) => Ok(store),
            None => Err(StoreError::SnapshotNotFound(snapshot)),
        });
        let store = match located {
            Ok(store) => store,
            Err(error) => {
                let _ = head_sender.send(Err(error));
                return;
            }
        };
        let content = match store.content(snapshot, &path) {
            Ok(content) => content,
            Err(error) => {
                let _ = head_sender.send(Err(error));
                return;
            }
        };

        let size = content.size();
        let tag = content.tag();
        let plan = plan(range.as_deref(), if_none_match.as_deref(), size, &tag);
        if head_sender.send(Ok(Head { size, tag, plan })).is_err() {
            return;
        }

        let (start, length) = match plan {
            Plan::Whole => (0, size),
            Plan::Partial { start, length } => (start, length),
            Plan::NotModified | Plan::Unsatisfiable => return,
        };
        let delivered = content.read(start, length, &mut |chunk| {
            chunks
                .blocking_send(Ok(Bytes::copy_from_slice(chunk)))
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
        });
        if let Err(error) = delivered {
            // A client that went away is not worth a log line; anything else ends the body with an
            // error, so the client sees a truncated transfer rather than a short file it believes.
            if !matches!(&error, StoreError::Delivery(io) if io.kind() == std::io::ErrorKind::BrokenPipe)
            {
                let _ = chunks.blocking_send(Err(std::io::Error::other(error.to_string())));
            }
        }
    });

    let head = head
        .await
        .map_err(|_| {
            ProblemResponse::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "history_unreadable",
                "the version could not be read",
            )
        })?
        .map_err(store_problem)?;

    let etag = HeaderValue::from_str(&format!("\"{}\"", head.tag))
        .expect("a hexadecimal tag is a valid header value");
    let common = [
        (header::ETAG, etag),
        (header::ACCEPT_RANGES, HeaderValue::from_static("bytes")),
    ];

    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });

    let response = match head.plan {
        Plan::NotModified => (StatusCode::NOT_MODIFIED, common).into_response(),
        Plan::Unsatisfiable => (
            StatusCode::RANGE_NOT_SATISFIABLE,
            common,
            [(
                header::CONTENT_RANGE,
                header_value(&format!("bytes */{}", head.size)),
            )],
        )
            .into_response(),
        Plan::Whole => (
            StatusCode::OK,
            common,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/octet-stream"),
                ),
                (header::CONTENT_LENGTH, header_value(&head.size.to_string())),
            ],
            Body::from_stream(stream),
        )
            .into_response(),
        Plan::Partial { start, length } => (
            StatusCode::PARTIAL_CONTENT,
            common,
            [
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/octet-stream"),
                ),
                (header::CONTENT_LENGTH, header_value(&length.to_string())),
                (
                    header::CONTENT_RANGE,
                    header_value(&format!(
                        "bytes {start}-{}/{}",
                        start + length - 1,
                        head.size
                    )),
                ),
            ],
            Body::from_stream(stream),
        )
            .into_response(),
    };

    Ok(response)
}

fn header_text(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn header_value(text: &str) -> HeaderValue {
    HeaderValue::from_str(text)
        .expect("digits, spaces, a dash and a slash are a valid header value")
}

#[cfg(test)]
mod tests {
    use super::{Plan, plan};

    const TAG: &str = "abc";

    #[test]
    fn no_range_is_the_whole_content() {
        assert_eq!(plan(None, None, 10, TAG), Plan::Whole);
    }

    #[test]
    fn a_closed_range_is_clamped_to_the_content() {
        assert_eq!(
            plan(Some("bytes=2-5"), None, 10, TAG),
            Plan::Partial {
                start: 2,
                length: 4
            }
        );
        assert_eq!(
            plan(Some("bytes=8-100"), None, 10, TAG),
            Plan::Partial {
                start: 8,
                length: 2
            }
        );
    }

    #[test]
    fn an_open_range_runs_to_the_end_and_a_suffix_counts_from_it() {
        assert_eq!(
            plan(Some("bytes=7-"), None, 10, TAG),
            Plan::Partial {
                start: 7,
                length: 3
            }
        );
        assert_eq!(
            plan(Some("bytes=-4"), None, 10, TAG),
            Plan::Partial {
                start: 6,
                length: 4
            }
        );
        assert_eq!(
            plan(Some("bytes=-40"), None, 10, TAG),
            Plan::Partial {
                start: 0,
                length: 10
            }
        );
    }

    #[test]
    fn a_range_outside_the_content_is_unsatisfiable() {
        assert_eq!(plan(Some("bytes=10-"), None, 10, TAG), Plan::Unsatisfiable);
        assert_eq!(plan(Some("bytes=-0"), None, 10, TAG), Plan::Unsatisfiable);
        assert_eq!(plan(Some("bytes=0-"), None, 0, TAG), Plan::Unsatisfiable);
    }

    #[test]
    fn what_cannot_be_honoured_simply_is_answered_whole() {
        for range in ["bytes=0-1,4-5", "lines=1-2", "bytes=5-2", "bytes=x-"] {
            assert_eq!(plan(Some(range), None, 10, TAG), Plan::Whole, "{range}");
        }
    }

    #[test]
    fn a_matching_tag_means_the_client_already_has_it() {
        assert_eq!(plan(None, Some("\"abc\""), 10, TAG), Plan::NotModified);
        assert_eq!(
            plan(None, Some("\"x\", \"abc\""), 10, TAG),
            Plan::NotModified
        );
        assert_eq!(
            plan(Some("bytes=0-1"), Some("*"), 10, TAG),
            Plan::NotModified
        );
        assert_eq!(plan(None, Some("\"other\""), 10, TAG), Plan::Whole);
    }
}
