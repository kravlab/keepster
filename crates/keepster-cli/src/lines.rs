//! A stream of lines from the daemon, as `GET /v1/preview` sends a Capture Preview (ticket 57): one JSON
//! object per line, read as it arrives so a long walk renders as it goes.

use http_body_util::BodyExt;
use hyper::body::Incoming;

use crate::client::ClientError;

/// An open stream of lines. Dropping it closes the connection, which is what stops the walk behind it.
#[derive(Debug)]
pub struct LineStream {
    body: Incoming,
    buffer: Vec<u8>,
}

impl LineStream {
    /// The lines of `body`, read as they arrive.
    pub(crate) fn new(body: Incoming) -> Self {
        Self {
            body,
            buffer: Vec::new(),
        }
    }

    /// The next line, without its line feed, waiting for it; `None` once the daemon ended the stream.
    pub async fn next(&mut self) -> Result<Option<String>, ClientError> {
        loop {
            if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=end).collect();
                return Ok(Some(
                    String::from_utf8_lossy(&line[..end])
                        .trim_end_matches('\r')
                        .to_owned(),
                ));
            }
            match self.body.frame().await {
                Some(frame) => {
                    if let Ok(data) = frame?.into_data() {
                        self.buffer.extend_from_slice(&data);
                    }
                }
                // A last line without its line feed is still a line.
                None if !self.buffer.is_empty() => {
                    let line = std::mem::take(&mut self.buffer);
                    return Ok(Some(
                        String::from_utf8_lossy(&line)
                            .trim_end_matches('\r')
                            .to_owned(),
                    ));
                }
                None => return Ok(None),
            }
        }
    }
}
