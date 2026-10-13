//! The loopback listener: the one network surface, and the browser's way in (ticket 60).
//!
//! The control channel needs no token, because OS permissions are its boundary. This listener does,
//! and the real threat is the browser: any page the user has open can send a request to the loopback,
//! and through DNS rebinding can claim its own origin while doing it. So the token never travels in a
//! URL — only a single-use bootstrap code does, exchanged for a cookie — and every request is checked
//! twice: `Host` must name this listener, and `Origin`, when there is one, must be an allowed one.

use std::collections::HashSet;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use keepster_api::{ErrorCode, ListenerState, ListenerStatus};

use crate::problem::ProblemResponse;

/// The file in the runtime directory holding the token for this start: what a third-party client
/// reads to authenticate, as `Authorization: Bearer <token>`, and what the browser's cookie carries.
pub const TOKEN_FILE: &str = "ui-token";

/// What the daemon serves without the token: what a client needs to tell a refused token from a
/// version it does not speak (ticket 07). `Host` and `Origin` are checked on them all the same.
const PUBLIC: &[&str] = &["/v1/version", "/v1/openapi.json"];

/// Whether the daemon has a network surface, decided once when it starts.
#[derive(Debug)]
pub(crate) enum Listener {
    /// `ui.listener` is off in the global configuration at `config_path`: nothing listens, and no
    /// token exists.
    Off {
        /// The file to switch it back on in, when the daemon has one.
        config_path: Option<PathBuf>,
    },
    /// It listens, behind this gate.
    Listening(Arc<Gate>),
    /// It is on and could not bind; why, for `status`.
    Failed(String),
}

impl Listener {
    /// The row `status` shows.
    pub(crate) fn status(&self) -> ListenerStatus {
        match self {
            Self::Off { .. } => ListenerStatus {
                state: ListenerState::Off,
                address: None,
                detail: None,
            },
            Self::Listening(gate) => ListenerStatus {
                state: ListenerState::Listening,
                address: Some(gate.address.to_string()),
                detail: None,
            },
            Self::Failed(why) => ListenerStatus {
                state: ListenerState::Failed,
                address: None,
                detail: Some(why.clone()),
            },
        }
    }

    /// The gate, or the problem a request needing one is refused with.
    pub(crate) fn gate(&self) -> Result<&Arc<Gate>, ProblemResponse> {
        // Naming the key, the file and the restart, because that is the whole way back (ticket 24): the
        // listener is never started for one session.
        let (reason, detail) = match self {
            Self::Listening(gate) => return Ok(gate),
            Self::Off { config_path } => (
                "off",
                format!(
                    "the daemon has no loopback listener: `ui.listener` is off in {}; set it to \
                     true there and run `keepster service restart`",
                    config_path.as_ref().map_or_else(
                        || "the global configuration".to_owned(),
                        |path| path.display().to_string()
                    )
                ),
            ),
            Self::Failed(_) => (
                "failed",
                "the daemon's loopback listener could not bind; `keepster status` says why"
                    .to_owned(),
            ),
        };

        Err(
            ProblemResponse::new(StatusCode::CONFLICT, ErrorCode::ListenerUnavailable, detail)
                .with_detail("reason", reason)
                .with_detail("key", "ui.listener"),
        )
    }
}

/// What every request to the listener passes through: where it is, which origins it answers, and the
/// token of this start.
#[derive(Debug)]
pub(crate) struct Gate {
    address: SocketAddr,
    /// The listener's own origins, then the configured ones, each as a browser spells `Origin`.
    origins: Vec<String>,
    /// Where the token is kept for the machine's owner to read.
    token_path: PathBuf,
    secrets: Mutex<Secrets>,
}

/// The token and the bootstrap codes not yet exchanged, which rotating forgets together.
#[derive(Debug)]
struct Secrets {
    token: String,
    codes: HashSet<String>,
}

impl Gate {
    /// The gate of a listener bound at `address`, answering `allowed` origins besides its own, with a
    /// token minted now and written to `token_path`.
    pub(crate) fn new(
        address: SocketAddr,
        allowed: &[String],
        token_path: PathBuf,
    ) -> io::Result<Self> {
        let port = address.port();
        let mut origins = vec![
            format!("http://127.0.0.1:{port}"),
            format!("http://localhost:{port}"),
        ];
        origins.extend(allowed.iter().cloned());
        let token = secret()?;
        write_token(&token_path, &token)?;

        Ok(Self {
            address,
            origins,
            token_path,
            secrets: Mutex::new(Secrets {
                token,
                codes: HashSet::new(),
            }),
        })
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    /// A new single-use code, and the URL a browser opens with it.
    pub(crate) fn bootstrap(&self) -> io::Result<String> {
        let code = secret()?;
        self.secrets().codes.insert(code.clone());

        Ok(format!(
            "http://127.0.0.1:{}/?bootstrap={code}",
            self.address.port()
        ))
    }

    /// Mint a new token, revoking every session the old one opened and every code not yet exchanged.
    pub(crate) fn rotate(&self) -> io::Result<()> {
        let token = secret()?;
        let mut secrets = self.secrets();
        // Written before it is in force, so a client that read the file is never ahead of the daemon.
        write_token(&self.token_path, &token)?;
        secrets.token = token;
        secrets.codes.clear();

        Ok(())
    }

    /// Take the token's file away once nothing is listening any more.
    pub(crate) fn withdraw(&self) {
        remove_token(&self.token_path);
    }

    /// The cookie's name. A cookie is scoped to a host and never to a port, so two daemons on one
    /// machine — two users — name theirs apart rather than overwrite each other's.
    fn cookie(&self) -> String {
        format!("keepster-{}", self.address.port())
    }

    fn secrets(&self) -> MutexGuard<'_, Secrets> {
        self.secrets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether `host` names this listener: the loopback address or `localhost`, with its port.
    fn host_allowed(&self, host: &str) -> bool {
        let port = self.address.port();
        host.eq_ignore_ascii_case(&format!("127.0.0.1:{port}"))
            || host.eq_ignore_ascii_case(&format!("localhost:{port}"))
    }

    fn origin_allowed(&self, origin: &str) -> bool {
        self.origins
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(origin))
    }

    /// Whether the request carries this start's token, in the cookie or as a bearer.
    fn authenticated(&self, headers: &HeaderMap) -> bool {
        let token = self.secrets().token.clone();
        let cookie = self.cookie();
        let from_cookie = headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .any(|(name, value)| name == cookie && same(value, &token));
        let from_bearer = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|value| same(value.trim(), &token));

        from_cookie || from_bearer
    }

    /// Exchange a bootstrap code for the cookie, burning it: a code already used, or never issued, is
    /// refused.
    fn exchange(&self, code: &str) -> Response {
        let token = {
            let mut secrets = self.secrets();
            if !secrets.codes.remove(code) {
                return refused(
                    StatusCode::UNAUTHORIZED,
                    ErrorCode::Unauthorized,
                    "this bootstrap code is not one the daemon issued, or it was already used; run \
                     `keepster ui` again",
                );
            }
            secrets.token.clone()
        };
        let cookie = format!(
            "{}={token}; HttpOnly; SameSite=Strict; Path=/",
            self.cookie()
        );

        // Redirected to a clean URL, so the code leaves the address bar at once; `no-referrer` keeps
        // it out of the one request the redirect makes.
        let mut response = (StatusCode::SEE_OTHER, [(header::LOCATION, "/")]).into_response();
        let headers = response.headers_mut();
        if let Ok(cookie) = HeaderValue::from_str(&cookie) {
            headers.insert(header::SET_COOKIE, cookie);
        }
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );

        response
    }
}

/// Every request to the loopback listener, before any handler sees it.
pub(crate) async fn guard(State(gate): State<Arc<Gate>>, request: Request, next: Next) -> Response {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    // An absolute-form request names its authority in the target too, and it must say the same.
    let named = request
        .uri()
        .authority()
        .map(|authority| authority.as_str());
    if !host.is_some_and(|host| gate.host_allowed(host))
        || named.is_some_and(|named| Some(named) != host)
    {
        return refused(
            StatusCode::FORBIDDEN,
            ErrorCode::HostRefused,
            "the request's Host is not this listener's own address and port",
        );
    }

    let origin = match headers.get(header::ORIGIN) {
        None => None,
        Some(origin) => match origin.to_str() {
            Ok(origin) if gate.origin_allowed(origin) => Some(origin.to_owned()),
            _ => {
                return refused(
                    StatusCode::FORBIDDEN,
                    ErrorCode::OriginRefused,
                    "the request's Origin is not this listener's own, nor one `ui.allowed_origins` names",
                );
            }
        },
    };

    if request.method() == Method::OPTIONS && origin.is_some() {
        return allowing(origin, preflight());
    }

    if request.uri().path() == "/" {
        if let Some(code) = bootstrap_code(request.uri().query()) {
            return gate.exchange(&code);
        }
    }

    if !PUBLIC.contains(&request.uri().path()) && !gate.authenticated(request.headers()) {
        return allowing(
            origin,
            refused(
                StatusCode::UNAUTHORIZED,
                ErrorCode::Unauthorized,
                "this listener answers a browser session opened by `keepster ui`, or a client \
                 sending the token from its file as `Authorization: Bearer`",
            ),
        );
    }

    allowing(origin, next.run(request).await)
}

/// The answer to a CORS preflight from an allowed origin: which methods and headers it may send.
fn preflight() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(
            "authorization, content-type, last-event-id, range, if-none-match",
        ),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );

    response
}

/// `response`, telling an allowed `origin` by name that it may read it — never `*`, which would tell
/// every page the user has open.
fn allowing(origin: Option<String>, mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    if let Some(origin) = origin.and_then(|origin| HeaderValue::from_str(&origin).ok()) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );
    }

    response
}

fn refused(status: StatusCode, code: ErrorCode, detail: &str) -> Response {
    ProblemResponse::new(status, code, detail).into_response()
}

/// The `bootstrap` parameter of a query, if it has one.
fn bootstrap_code(query: Option<&str>) -> Option<String> {
    query?
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == "bootstrap")
        .map(|(_, code)| code.to_owned())
}

/// Whether two secrets are equal, taking as long whichever byte differs: an early return would tell a
/// patient local process how much of its guess was right.
fn same(given: &str, secret: &str) -> bool {
    given.len() == secret.len()
        && given
            .bytes()
            .zip(secret.bytes())
            .fold(0u8, |differ, (a, b)| differ | (a ^ b))
            == 0
}

/// 256 bits from the operating system's generator, as hex.
fn secret() -> io::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;

    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Write `token` at `path`, readable by its owner alone, replacing whatever was there in one step.
fn write_token(path: &Path, token: &str) -> io::Result<()> {
    use std::io::Write;

    if let Some(directory) = path.parent() {
        create_private_dir(directory)?;
    }
    let temporary = path.with_extension("new");
    remove_token(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Created with the mode rather than narrowed after, so no moment exists where it is readable.
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    drop(file);

    std::fs::rename(&temporary, path)
}

/// Remove a token file, if there is one: a secret guarding nothing is a liability, not a convenience.
pub(crate) fn remove_token(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(file = %path.display(), %error, "a token file could not be removed");
        }
    }
}

fn create_private_dir(directory: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }

    builder.create(directory)
}
