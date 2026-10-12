//! A client for the loopback listener that sends exactly what a test says — including the `Host` and
//! `Origin` a hostile page would — which no well-behaved client library does by default.

use std::net::SocketAddr;

use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use keepster_cli::Response;

/// Send one request to `address` with `headers` and no others, and read the whole answer.
pub(crate) async fn request(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<Response, Box<dyn std::error::Error + Send + Sync>> {
    let stream = tokio::net::TcpStream::connect(address).await?;
    let (mut sender, connection) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await?;
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = sender
        .send_request(request.body(Full::new(Bytes::new()))?)
        .await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.into_body().collect().await?.to_bytes().to_vec();

    Ok(Response {
        status,
        headers,
        body,
    })
}
