//! A stream of JSON objects, one per line (`application/x-ndjson`), written by a blocking walk and read by
//! a client at its own pace: what a Capture Preview and a content search answer with (tickets 57, 59).
//!
//! The walk stops the moment its reader goes away, and gives up on one that stops reading, so neither a
//! closed tab nor a stalled client keeps the daemon walking.

use std::convert::Infallible;
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// How many lines may wait for a reader that is slower than the walk.
const QUEUED_LINES: usize = 64;

/// How long a line waits for a reader that has stopped reading before the walk gives up on it: ample for
/// one that is merely slow, and short enough that one which stopped does not hold the walk for ever.
pub(crate) const STALL: Duration = Duration::from_secs(60);

/// The two ends of a stream: the sender the walk writes to, and the response that reads from it.
pub(crate) fn stream() -> (mpsc::Sender<Bytes>, Response) {
    let (lines, received) = mpsc::channel::<Bytes>(QUEUED_LINES);
    let body = futures_util::stream::unfold(received, |mut received| async move {
        received
            .recv()
            .await
            .map(|line| (Ok::<_, Infallible>(line), received))
    });
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from_stream(body))
        .expect("a response with a valid header");

    (lines, response)
}

/// Send `record` down `lines` as one line, waiting at most `stall` for room: whether anyone is still
/// reading.
pub(crate) fn send<T: Serialize>(lines: &mpsc::Sender<Bytes>, record: &T, stall: Duration) -> bool {
    let mut line = serde_json::to_vec(record).expect("a record serialises");
    line.push(b'\n');
    let mut line = Bytes::from(line);
    let deadline = Instant::now() + stall;
    loop {
        match lines.try_send(line) {
            Ok(()) => return true,
            Err(TrySendError::Closed(_)) => return false,
            Err(TrySendError::Full(_)) if Instant::now() >= deadline => return false,
            Err(TrySendError::Full(back)) => {
                line = back;
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
