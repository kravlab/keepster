//! The daemon: the only process that touches a History Store, and the only crate allowed to read a
//! clock (ADR 0012).
//!
//! One HTTP API is served on the control channel — a permission-protected socket on Unix, a named
//! pipe on Windows — and, from ticket 60, on a loopback listener for the browser. Everything below
//! this crate takes the time it needs as a parameter.

// A contract mechanism, not a style choice: a handler that is never passed to `routes!` is neither
// routed nor documented, and this is what turns that silent omission into a build failure (ticket 40).
#![deny(dead_code)]

mod configuration;
mod content;
mod events;
mod explain;
mod openapi;
mod problem;
mod roots;
mod snapshots;
mod triggers;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use keepster_core::RootId;

use axum::extract::State;
use axum::{Json, Router};
use keepster_api::{API_VERSION, DaemonStatus, Problem, Reported, Status, VersionInfo};
use keepster_store::Registry;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub use openapi::openapi_json;

/// How to run a daemon instance.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Where the control channel is created. Tests use a short temporary path deliberately:
    /// `sun_path` holds 104 bytes on macOS and 108 on Linux.
    pub control_path: PathBuf,
    /// The directory that holds one subdirectory per Watched Root: its Registry record and, from
    /// ticket 38, its History Store.
    pub store_base: PathBuf,
    /// The global configuration file, or `None` to run without one. It need not exist: a user who
    /// configures nothing gets the documented defaults.
    pub config_path: Option<PathBuf>,
    /// How many directory watches all Roots together may hold, or `None` for the platform's own ceiling.
    /// A Root that would exceed it is reconciled periodically instead of watched (ticket 44).
    pub watch_ceiling: Option<usize>,
}

/// A daemon that is listening, and the handle that stops it.
///
/// Dropping it asks the daemon to stop; awaiting [`RunningDaemon::shutdown`] waits until it has.
pub struct RunningDaemon {
    control_path: PathBuf,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<io::Result<()>>,
}

impl RunningDaemon {
    /// The control channel this daemon is listening on.
    pub fn control_path(&self) -> &Path {
        &self.control_path
    }

    /// Stop serving and wait for the listener to close, returning whatever the server returned.
    ///
    /// Ticket 61 extends this into the full stop sequence: drain the capture in flight, then flush
    /// every pending debounce window.
    pub async fn shutdown(mut self) -> io::Result<()> {
        if let Some(signal) = self.shutdown.take() {
            let _ = signal.send(());
        }
        match (&mut self.task).await {
            Ok(result) => result,
            Err(joined) => Err(io::Error::other(joined)),
        }
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        if let Some(signal) = self.shutdown.take() {
            let _ = signal.send(());
        }
    }
}

/// Bind the control channel and start serving, returning once the channel is connectable.
///
/// Binding happens before the task is spawned, so a caller that gets an `Ok` can connect
/// immediately without polling for readiness.
pub async fn start(config: DaemonConfig) -> io::Result<RunningDaemon> {
    let registry = Registry::open(&config.store_base).map_err(io::Error::other)?;
    let registry = tokio::task::spawn_blocking(move || {
        recover_indexes(&registry);
        registry
    })
    .await
    .map_err(io::Error::other)?;
    let listener = bind_control_channel(&config.control_path)?;
    let (shutdown, stop) = oneshot::channel();
    let ceiling = config
        .watch_ceiling
        .or_else(keepster_watch::default_ceiling);
    let state = DaemonState::new(registry, config.config_path, config.store_base, ceiling);
    // The daemon's first look at the Registry happens before any request is served, so that the start's
    // Reconciliation is told apart from the one a registration owes: a Root registered a moment later is
    // a Registry change, never the start.
    let starting = state.clone();
    tokio::task::spawn_blocking(move || configuration::reload(&starting))
        .await
        .map_err(io::Error::other)?;
    // Every file is read again whenever a request needs it; the tick is what makes a reload happen
    // when nothing asks, so that what follows from one — a Root queued for a full comparison, a
    // `Degraded` state — does not wait for a client.
    let ticker = tokio::spawn(configuration::reload_continually(state.clone()));
    let task = tokio::spawn(async move {
        let events = Arc::clone(&state.events);
        let stopping = async move {
            let _ = stop.await;
            // An open event stream would otherwise hold the graceful stop forever.
            events.close();
        };
        let served = serve(listener, router(state.clone()), stopping).await;
        // No further Trigger is accepted; the captures in flight finish and every pending debounce
        // window is captured before the daemon is done (ticket 05).
        ticker.abort();
        state.runtimes.stop().await;
        served
    });

    Ok(RunningDaemon {
        control_path: config.control_path,
        shutdown: Some(shutdown),
        task,
    })
}

/// Bring every Root's derived index up to date before anything is served.
///
/// This is where a daemon killed between a ref update and an index write is repaired: the watermark
/// shows how far the index got, and the gap is indexed by the same code that rebuilds a deleted index
/// (ticket 03). It runs before the control channel exists, so no request can hold the index open while
/// a damaged one is discarded — deleting a database another connection holds is itself a corruption
/// vector. A Root whose history cannot be recovered is reported and passed over: its reads will say
/// why, and one Root must not keep the daemon from serving the others.
fn recover_indexes(registry: &Registry) {
    let records = match registry.readable() {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "the Registry could not be read to recover history indexes");
            return;
        }
    };

    for record in records {
        let location = keepster_app::snapshots::location(&registry.root_dir(record.id));
        if let Err(error) = keepster_app::snapshots::recover(&location) {
            tracing::warn!(root = %record.id, %error, "the history index could not be recovered");
        }
    }
}

/// What this build tells a client about itself.
pub fn version_info() -> VersionInfo {
    VersionInfo {
        build_version: env!("CARGO_PKG_VERSION").to_owned(),
        build_hash: build_hash().to_owned(),
        api_version: API_VERSION.to_owned(),
    }
}

/// The commit of this build, or `unknown` when it was built without a repository.
///
/// The hash is recorded in full: an abbreviation's length depends on repository size and clone
/// depth, so it is not a stable identifier. Anything that is not a full hash — including vergen's
/// idempotent placeholder — is reported as `unknown` rather than passed through.
fn build_hash() -> &'static str {
    const FULL_HASH_LENGTH: usize = 40;

    match option_env!("VERGEN_GIT_SHA") {
        Some(hash) if hash.len() == FULL_HASH_LENGTH => hash,
        _ => "unknown",
    }
}

/// Everything the handlers share. Cheap to clone, because axum hands each request a copy.
#[derive(Debug, Clone)]
struct DaemonState {
    /// When this daemon started serving. Monotonic: uptime must not move when the wall clock does,
    /// and ADR 0012 keeps wall-clock reads out of everything below this crate anyway.
    started: Instant,
    /// The Registry of Watched Roots, on disk, behind the exclusion that makes its rules hold.
    registry: Arc<Mutex<Registry>>,
    /// One lock per History Store, so that a capture and — from ticket 52 — a maintenance pass are
    /// never in the same store at once.
    store_locks: Arc<Mutex<HashMap<RootId, Arc<Mutex<()>>>>>,
    /// The configuration in force for every Root, re-read whenever its files change.
    config: Arc<Mutex<keepster_app::config::Configuration>>,
    /// Every Root's automatic capture.
    runtimes: Arc<triggers::Runtimes>,
    /// What happened, for every open event stream and the ones that reconnect.
    events: Arc<events::EventBus>,
    /// What the event stream last said about each Root's state and configuration.
    announced: Arc<Mutex<events::Announced>>,
}

impl DaemonState {
    fn new(
        registry: Registry,
        config_path: Option<PathBuf>,
        store_base: PathBuf,
        watch_ceiling: Option<usize>,
    ) -> Self {
        Self {
            started: Instant::now(),
            registry: Arc::new(Mutex::new(registry)),
            store_locks: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(keepster_app::config::Configuration::new(
                config_path,
                std::env::home_dir(),
                store_base,
            ))),
            runtimes: Arc::new(triggers::Runtimes::new(watch_ceiling)),
            events: Arc::new(events::EventBus::new(events::BOUND)),
            announced: Arc::new(Mutex::new(events::Announced::default())),
        }
    }

    /// Tell every open event stream that `event` happened.
    fn publish(&self, event: keepster_api::Event) {
        self.events.publish(
            keepster_core::EventId::generate(roots::now()),
            Instant::now(),
            event,
        );
    }

    /// The configuration of the Root `id` at `root_path` as it stands, re-reading whichever of its
    /// files changed since it was last asked for.
    fn configuration(&self, id: RootId, root_path: &Path) -> keepster_app::config::Resolved {
        self.config().resolve(id, root_path)
    }

    /// The configuration in force for the Root `id` at `root_path`, or the problem that keeps it from
    /// being captured at all: a file that never had a valid version (ticket 04).
    fn effective(
        &self,
        id: RootId,
        root_path: &Path,
    ) -> Result<keepster_app::config::Effective, problem::ProblemResponse> {
        let resolved = self.configuration(id, root_path);

        resolved
            .effective
            .ok_or_else(|| problem::config_problem(&resolved.errors))
    }

    /// Let go of the configuration of every Root not among `registered`, once it is deregistered.
    fn forget_configuration_except(&self, registered: &[keepster_store::RootRecord]) {
        self.config()
            .retain(|id| registered.iter().any(|record| record.id == id));
    }

    /// The Roots whose classification inputs changed since this was last asked.
    fn take_reconcile_queue(&self) -> std::collections::BTreeSet<RootId> {
        self.config().take_reconcile_queue()
    }

    /// The configuration, held for the length of one resolution.
    ///
    /// Taken after the Registry wherever both are held, never before, so the two locks cannot
    /// deadlock. The whole order is: a store's lock, the Registry, the configuration, the runtimes, then what
    /// the event stream announced and its buffer. A panic while it is held loses nothing that matters: every file is compared with what
    /// was read last, so the next resolution reads again whatever a panic left half-done.
    fn config(&self) -> MutexGuard<'_, keepster_app::config::Configuration> {
        self.config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The exclusive lock on one Root's History Store.
    ///
    /// Held by a capture or, from ticket 52, by a maintenance pass, and never by both: git's own
    /// documentation concedes a corruption risk under concurrent access, and two captures racing would
    /// leave one of them unreferenced in any case (ticket 06). One lock per store rather than one for
    /// all of them, because an unrelated Root has no reason to wait.
    fn store_lock(&self, id: RootId) -> Arc<Mutex<()>> {
        let mut locks = self
            .store_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        Arc::clone(locks.entry(id).or_default())
    }

    /// The Registry, held for the length of one operation.
    ///
    /// Registering is list, then check for an overlap, then insert — which is only a rule if nothing
    /// interleaves, or two requests arriving together would both pass the check and produce the nested
    /// Roots ticket 07 forbids. The daemon is the only process that touches this state (ticket 05), so
    /// one mutex is the whole exclusion.
    ///
    /// A panic while the lock is held does not disable the daemon: the Registry's files are replaced
    /// atomically, so there is no half-updated state for a poisoned lock to protect.
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn router(state: DaemonState) -> Router {
    let (router, _) = api().split_for_parts();

    router.with_state(state)
}

/// Every route, each with its description.
///
/// `routes!` takes the path from the handler's own `#[utoipa::path]` and hands the same string to axum,
/// so the route and the documented path are one literal: renaming a handler fails to compile, and a
/// handler left out is dead code. Handlers are grouped by path, because one `routes!` registers one
/// path.
fn api() -> OpenApiRouter<DaemonState> {
    OpenApiRouter::with_openapi(openapi::base())
        .routes(routes!(openapi::serve))
        .routes(routes!(version))
        .routes(routes!(status))
        .routes(routes!(events::stream))
        .routes(routes!(roots::list, roots::register))
        .routes(routes!(roots::show, roots::deregister, roots::move_root))
        .routes(routes!(snapshots::list, snapshots::create))
        .routes(routes!(snapshots::reconcile))
        .routes(routes!(snapshots::entries))
        .routes(routes!(snapshots::content))
        .routes(routes!(explain::explain))
        .routes(routes!(configuration::show))
}

/// Which build and which API this daemon is.
#[utoipa::path(
    get,
    path = "/v1/version",
    operation_id = "get_version",
    tag = "contract",
    responses((status = 200, description = "The build and the API it speaks. Served before authentication.", body = VersionInfo)),
)]
async fn version() -> Json<VersionInfo> {
    Json(version_info())
}

/// The whole health surface.
#[utoipa::path(
    get,
    path = "/v1/status",
    operation_id = "get_status",
    tag = "status",
    responses(
        (status = 200, description = "The daemon and every Watched Root. `keepster status --json` prints exactly this.", body = Status),
        (status = 500, description = "The Registry could not be read.", body = Problem, content_type = "application/problem+json"),
    ),
)]
async fn status(
    State(state): State<DaemonState>,
) -> Result<Json<Status>, problem::ProblemResponse> {
    let registered = roots::reported(&state, &state.registry())?;

    Ok(Json(Status {
        daemon: DaemonStatus {
            version: version_info(),
            pid: std::process::id(),
            uptime_seconds: state.started.elapsed().as_secs(),
            // Reported as unknown until the tickets that can determine these land: ticket 61 for
            // autostart and linger, ticket 52 for maintenance.
            autostart: Reported::Unknown,
            linger: Reported::Unknown,
            maintenance: Reported::Unknown,
        },
        roots: registered,
    }))
}

#[cfg(unix)]
fn bind_control_channel(path: &Path) -> io::Result<tokio::net::UnixListener> {
    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        fs::set_permissions(parent, Permissions::from_mode(0o700))?;
    }

    // A socket file left by a crash is stale precisely when nothing is listening on it. Ticket 61
    // makes the flock'd pid file the authority on that, and unlinks only once the lock proves free;
    // until then the daemon assumes the path is its own.
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let listener = tokio::net::UnixListener::bind(path)?;
    fs::set_permissions(path, Permissions::from_mode(0o600))?;

    Ok(listener)
}

#[cfg(windows)]
fn bind_control_channel(path: &Path) -> io::Result<pipe::PipeListener> {
    pipe::PipeListener::bind(path)
}

/// Serve until the stop signal arrives, whatever the platform's control channel is.
///
/// One function over both listeners: only the binding differs per platform, and the serving does not.
async fn serve<L>(
    listener: L,
    router: Router,
    stop: impl Future<Output = ()> + Send + 'static,
) -> io::Result<()>
where
    L: axum::serve::Listener,
    L::Addr: std::fmt::Debug,
{
    axum::serve(listener, router)
        .with_graceful_shutdown(stop)
        .await
}

/// The Windows control channel, which is a named pipe rather than a socket file.
///
/// A pipe has no file to bind and unlink: each accepted connection consumes one *instance*, so the
/// listener always keeps the next instance ready. All of the Win32 work — the owner's SID, the
/// security descriptor, the creation call — lives in `keepster-service`, which keeps every `unsafe`
/// block in the workspace in one module.
#[cfg(windows)]
mod pipe {
    use std::ffi::OsString;
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    use axum::serve::Listener;
    use keepster_service::{OwnerOnlyAccess, create_control_pipe, current_user_sid};
    use tokio::net::windows::named_pipe::NamedPipeServer;

    /// How long to wait before retrying a failed instance creation, so a transient failure does not
    /// become a busy loop.
    const RETRY_DELAY: Duration = Duration::from_millis(50);

    pub(crate) struct PipeListener {
        name: OsString,
        access: OwnerOnlyAccess,
        /// The instance clients can connect to right now.
        ready: Option<NamedPipeServer>,
    }

    impl PipeListener {
        /// Create the pipe and its first instance.
        ///
        /// The first instance is created with the first-instance flag, which is what makes a second
        /// daemon fail here instead of quietly serving the same name beside this one — the pipe's own
        /// half of the single-instance rule ticket 61 completes with the pid file's lock.
        pub(crate) fn bind(path: &Path) -> io::Result<Self> {
            let name = path.as_os_str().to_os_string();
            let access = OwnerOnlyAccess::for_sid(&current_user_sid()?)?;
            let ready = create_control_pipe(&name, &access, true)?;

            Ok(Self {
                name,
                access,
                ready: Some(ready),
            })
        }
    }

    impl Listener for PipeListener {
        type Io = NamedPipeServer;
        /// A pipe connection has no peer address to report.
        type Addr = ();

        async fn accept(&mut self) -> (Self::Io, Self::Addr) {
            loop {
                let instance = match self.ready.take() {
                    Some(instance) => instance,
                    None => match create_control_pipe(&self.name, &self.access, false) {
                        Ok(instance) => instance,
                        Err(error) => {
                            tracing::warn!(%error, "could not create a control pipe instance");
                            tokio::time::sleep(RETRY_DELAY).await;
                            continue;
                        }
                    },
                };

                if let Err(error) = instance.connect().await {
                    tracing::warn!(%error, "a control pipe connection failed");
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }

                // The next instance is created before this one is handed over, so a client arriving
                // while the previous connection is still being served is not told the pipe is busy.
                match create_control_pipe(&self.name, &self.access, false) {
                    Ok(next) => self.ready = Some(next),
                    Err(error) => {
                        tracing::warn!(%error, "could not prepare the next control pipe instance");
                    }
                }

                return (instance, ());
            }
        }

        fn local_addr(&self) -> io::Result<Self::Addr> {
            Ok(())
        }
    }
}
