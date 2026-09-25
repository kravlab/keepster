//! The client half of "the CLI is a thin client over the same API" (ticket 07).
//!
//! It speaks ordinary HTTP over the control channel, so the CLI exercises exactly the contract a
//! third-party client sees. The control channel is protected by OS permissions rather than by a
//! token, so no credential handling appears here.

use std::path::{Path, PathBuf};

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;

/// A client bound to one daemon's control channel.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    control_path: PathBuf,
}

/// What the daemon answered: the status line's code and the whole body.
///
/// Bodies here are small (`status`, `version`); the endpoints that stream or serve file content get
/// their own path in the tickets that add them.
#[derive(Debug, Clone)]
pub struct Response {
    /// The HTTP status, the coarse layer of the error model (ticket 07).
    pub status: StatusCode,
    /// The response body, unparsed, so a caller can pass it through verbatim.
    pub body: Vec<u8>,
}

/// Why a request did not produce a response.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The control channel could not be reached: no daemon is listening, or it is not ours to talk
    /// to. This is the condition behind the CLI's "daemon unreachable" exit code.
    #[error("no daemon is listening on {path}")]
    Unreachable {
        /// The control channel that was tried.
        path: PathBuf,
        /// The underlying connection error.
        #[source]
        source: std::io::Error,
    },
    /// The connection was made but the exchange failed.
    #[error("the daemon did not answer: {0}")]
    Exchange(#[from] hyper::Error),
    /// The request could not be constructed, which is a programming error in the CLI.
    #[error("malformed request: {0}")]
    Request(#[from] hyper::http::Error),
}

impl DaemonClient {
    /// A client for the daemon listening on `control_path`.
    pub fn new(control_path: impl Into<PathBuf>) -> Self {
        Self {
            control_path: control_path.into(),
        }
    }

    /// Perform a `GET` against the daemon.
    pub async fn get(&self, path: &str) -> Result<Response, ClientError> {
        self.send("GET", path, None).await
    }

    /// Perform a `POST` of a JSON body against the daemon.
    pub async fn post(&self, path: &str, body: &[u8]) -> Result<Response, ClientError> {
        self.send("POST", path, Some(body)).await
    }

    /// Perform a `PATCH` of a JSON body against the daemon.
    pub async fn patch(&self, path: &str, body: &[u8]) -> Result<Response, ClientError> {
        self.send("PATCH", path, Some(body)).await
    }

    /// Perform a `DELETE` against the daemon.
    pub async fn delete(&self, path: &str) -> Result<Response, ClientError> {
        self.send("DELETE", path, None).await
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
    ) -> Result<Response, ClientError> {
        let stream = connect(&self.control_path).await?;
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(ClientError::Exchange)?;

        // The connection task drives the exchange; it ends when the response body is complete.
        tokio::spawn(async move {
            let _ = connection.await;
        });

        // HTTP/1.1 requires a Host header, and the control channel has no meaningful authority.
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(hyper::header::HOST, "keepster.local");
        if body.is_some() {
            request = request.header(hyper::header::CONTENT_TYPE, "application/json");
        }
        let request = request.body(Full::new(Bytes::copy_from_slice(body.unwrap_or_default())))?;

        let response = sender.send_request(request).await?;
        let status = response.status();
        let body = response.into_body().collect().await?.to_bytes().to_vec();

        Ok(Response { status, body })
    }
}

#[cfg(unix)]
async fn connect(path: &Path) -> Result<tokio::net::UnixStream, ClientError> {
    tokio::net::UnixStream::connect(path)
        .await
        .map_err(|source| ClientError::Unreachable {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(windows)]
async fn connect(
    path: &Path,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient, ClientError> {
    use std::time::Duration;

    use tokio::net::windows::named_pipe::ClientOptions;

    /// `ERROR_PIPE_BUSY`: the pipe exists, but every instance is serving someone else at this
    /// instant. The daemon prepares the next instance immediately after each connection, so this is a
    /// window of microseconds rather than a condition to report.
    const PIPE_BUSY: i32 = 231;
    const BUSY_RETRY_DELAY: Duration = Duration::from_millis(50);
    const BUSY_ATTEMPTS: u32 = 20;

    for _ in 0..BUSY_ATTEMPTS {
        match ClientOptions::new().open(path) {
            Ok(client) => return Ok(client),
            Err(error) if error.raw_os_error() == Some(PIPE_BUSY) => {
                tokio::time::sleep(BUSY_RETRY_DELAY).await;
            }
            Err(source) => {
                return Err(ClientError::Unreachable {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
    }

    Err(ClientError::Unreachable {
        path: path.to_path_buf(),
        source: std::io::Error::from_raw_os_error(PIPE_BUSY),
    })
}
