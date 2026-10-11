//! The client end of the daemon's live event stream, `GET /v1/events` (ticket 45).
//!
//! Server-sent events read off the same control channel as every other request, so `keepster log
//! --follow` consumes the very endpoint a browser does.

use http_body_util::BodyExt;
use hyper::body::{Bytes, Incoming};

use crate::client::ClientError;

/// One event as it arrived: its name, its identity and its JSON payload, unparsed.
///
/// Unparsed because the set of events grows inside `/v1`: a reader takes the ones it knows and passes
/// over the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field, the event's name.
    pub event: String,
    /// The `id:` field, what a reconnect sends back as `Last-Event-ID`; absent from an event that
    /// carries none.
    pub id: Option<String>,
    /// The `data:` lines, joined.
    pub data: String,
}

/// An open stream of events.
#[derive(Debug)]
pub struct EventStream {
    body: Incoming,
    buffer: Vec<u8>,
}

impl EventStream {
    pub(crate) fn new(body: Incoming) -> Self {
        Self {
            body,
            buffer: Vec::new(),
        }
    }

    /// The next event, waiting for it; `None` once the daemon closed the stream.
    pub async fn next(&mut self) -> Result<Option<SseEvent>, ClientError> {
        loop {
            while let Some(block) = self.take_block() {
                if let Some(event) = parse(&block) {
                    return Ok(Some(event));
                }
            }
            match self.body.frame().await {
                Some(frame) => {
                    if let Ok(data) = frame?.into_data() {
                        self.push(&data);
                    }
                }
                None => return Ok(None),
            }
        }
    }

    fn push(&mut self, data: &Bytes) {
        // A carriage return ends a line as a line feed does; normalising here keeps the parser to one.
        let text = data.iter().copied().filter(|byte| *byte != b'\r');
        self.buffer.extend(text);
    }

    /// The text up to the next blank line, which is what ends an event.
    fn take_block(&mut self) -> Option<String> {
        let end = self.buffer.windows(2).position(|pair| pair == b"\n\n")?;
        let block: Vec<u8> = self.buffer.drain(..end + 2).collect();

        Some(String::from_utf8_lossy(&block[..end]).into_owned())
    }
}

/// One block of lines as an event, or `None` for a block with no data — a keep-alive comment.
fn parse(block: &str) -> Option<SseEvent> {
    let mut event = None;
    let mut id = None;
    let mut data: Option<String> = None;

    for line in block.lines() {
        if line.starts_with(':') {
            continue;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => event = Some(value.to_owned()),
            "id" => id = Some(value.to_owned()),
            "data" => match &mut data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => data = Some(value.to_owned()),
            },
            _ => {}
        }
    }

    Some(SseEvent {
        event: event.unwrap_or_else(|| "message".to_owned()),
        id,
        data: data?,
    })
}
