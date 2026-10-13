//! The daemon: the only process that touches a History Store, and the only crate allowed to read a
//! clock (ADR 0012).
//!
//! One HTTP API is served on the control channel — a permission-protected socket on Unix, a named
//! pipe on Windows — and, from ticket 60, on a loopback listener for the browser. Everything below
//! this crate takes the time it needs as a parameter.

// A contract mechanism, not a style choice: a handler that is never passed to `routes!` is neither
// routed nor documented, and this is what turns that silent omission into a build failure (ticket 40).
#![deny(dead_code)]

mod alarms;
mod autostart;
pub mod bundle;
mod configuration;
mod content;
pub mod diagnostics;
mod diff;
mod directories;
mod events;
mod explain;
mod export;
mod git;
mod housekeeping;
mod jobs;
mod logging;
mod loopback;
mod ndjson;
mod openapi;
mod pins;
mod places;
mod preferences;
mod preview;
mod problem;
mod restore;
mod roots;
mod search;
mod snapshots;
mod trays;
mod triggers;
mod ui;
mod volumes;
#[cfg(feature = "embed-ui")]
mod web;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use keepster_core::RootId;

use axum::extract::State;
use axum::{Json, Router};
use keepster_api::{API_VERSION, DaemonStatus, Problem, Status, StoreFormat, VersionInfo};
use keepster_store::Registry;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub use housekeeping::HousekeepingTiming;
pub use logging::{FileLog, LOG_FILE};
pub use loopback::TOKEN_FILE;
pub use openapi::openapi_json;
pub use volumes::{PlatformVolumes, Volume, Volumes};

/// How to run a daemon instance.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Where the control channel is created. Tests use a short temporary path deliberately:
    /// `sun_path` holds 104 bytes on macOS and 108 on Linux.
    pub control_path: PathBuf,
    /// The per-user runtime directory, private to its owner, where the loopback listener's token is
    /// written for each start as [`TOKEN_FILE`] (ticket 60).
    pub runtime_dir: PathBuf,
    /// The port the loopback listener binds whatever `ui.port` says, or `None` to take it from the
    /// configuration. Only a test has reason to set it, to `0` for any free port.
    pub loopback_port: Option<u16>,
    /// The directory that holds one subdirectory per Watched Root: its Registry record and, from
    /// ticket 38, its History Store.
    pub store_base: PathBuf,
    /// The global configuration file, or `None` to run without one. It need not exist: a user who
    /// configures nothing gets the documented defaults.
    pub config_path: Option<PathBuf>,
    /// How many directory watches all Roots together may hold, or `None` for the platform's own ceiling.
    /// A Root that would exceed it is reconciled periodically instead of watched (ticket 44).
    pub watch_ceiling: Option<usize>,
    /// How long after an incomplete pass a Root is first passed over again, or `None` for
    /// [`keepster_core::RETRY_AFTER`]. Only a test has reason to set it.
    pub retry_after: Option<std::time::Duration>,
    /// How Retention and maintenance are paced, or `None` for [`HousekeepingTiming::default`]. Only a
    /// test has reason to set it.
    pub housekeeping: Option<HousekeepingTiming>,
    /// The log file the binary writes, installed once the configuration says how (ticket 62), or `None`
    /// to write none — a daemon a test starts in-process, which shares its process with others.
    pub log: Option<Arc<FileLog>>,
    /// Who answers which volume a path is on, or `None` for [`PlatformVolumes`]. Only a test has reason
    /// to set it: unplugging a disk is not something a test can do to the machine it runs on.
    pub volumes: Option<Arc<dyn Volumes>>,
}

/// A daemon that is listening, and the handle that stops it.
///
/// Dropping it asks the daemon to stop; awaiting [`RunningDaemon::shutdown`] waits until it has.
pub struct RunningDaemon {
    control_path: PathBuf,
    loopback: Option<std::net::SocketAddr>,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<io::Result<()>>,
}

impl RunningDaemon {
    /// The control channel this daemon is listening on.
    pub fn control_path(&self) -> &Path {
        &self.control_path
    }

    /// Where the loopback listener is, or `None` when there is none.
    pub fn loopback(&self) -> Option<std::net::SocketAddr> {
        self.loopback
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
pub async fn start(config: DaemonConfig) -> Result<RunningDaemon, StartError> {
    // First of all: index recovery below may discard a damaged index, which beside a running daemon
    // would unlink a database that daemon holds open, and the control channel is rebound after it. Only
    // the lock's holder may do either (ticket 61).
    let pid_lock = lock_pid_file(&config.runtime_dir.join(keepster_service::PID_FILE)).await?;
    let registry = Registry::open(&config.store_base).map_err(io::Error::other)?;
    let registry = tokio::task::spawn_blocking(move || {
        // Before anything is served, so no request races the rewrite (ADR 0010).
        if let Err(error) = registry.upgrade() {
            tracing::warn!(%error, "an older Registry record could not be rewritten in this version");
        }
        recover_indexes(&registry);
        registry
    })
    .await
    .map_err(io::Error::other)?;
    let socket = bind_control_channel(&config.control_path)?;
    let (shutdown, stop) = oneshot::channel();
    let ceiling = config
        .watch_ceiling
        .or_else(keepster_watch::default_ceiling);
    let config_path = config.config_path.clone();
    let mut configuration = keepster_app::config::Configuration::new(
        config.config_path,
        std::env::home_dir(),
        config.store_base.clone(),
    );
    let settings = configuration.daemon();
    if let Some(log) = &config.log {
        // A daemon that cannot write its log still captures: the insurance is the point, the log only
        // explains it. The journal hears why.
        if let Err(error) = log.install(&settings) {
            eprintln!("keepster: the log could not be opened, so nothing is logged: {error}");
        }
    }
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        pid = std::process::id(),
        "the daemon is starting"
    );
    let events = Arc::new(events::EventBus::new(events::REPLAY_BOUND));
    let log_levels = Arc::new(logging::LogLevels::new(
        settings.log_level.clone(),
        config.log.clone(),
        Arc::clone(&events),
    ));
    let token_path = config.runtime_dir.join(TOKEN_FILE);
    // Whatever a previous start left: a token is this start's or none, and a start that ends up with
    // no listener — switched off, or failing to bind — leaves no secret guarding nothing.
    loopback::remove_token(&token_path);
    let (listener, tcp) = if settings.listener {
        let port = config.loopback_port.unwrap_or(settings.port);
        match bind_loopback(port, &settings.allowed_origins, token_path).await {
            Ok((gate, tcp)) => (loopback::Listener::Listening(gate), Some(tcp)),
            Err(error) => {
                tracing::warn!(%error, port, "the loopback listener could not be started");
                (loopback::Listener::Failed(error.to_string()), None)
            }
        }
    } else {
        (loopback::Listener::Off { config_path }, None)
    };
    let state = DaemonState::new(
        registry,
        configuration,
        listener,
        log_levels,
        events,
        config.store_base,
        ceiling,
        config.retry_after.unwrap_or(keepster_core::RETRY_AFTER),
        config.housekeeping.unwrap_or_default(),
        config.volumes.unwrap_or_else(|| Arc::new(PlatformVolumes)),
    );
    // The daemon's first look at the Registry happens before any request is served, so that the start's
    // Reconciliation is told apart from the one a registration owes: a Root registered a moment later is
    // a Registry change, never the start.
    let starting = state.clone();
    tokio::task::spawn_blocking(move || {
        configuration::reload(&starting);
        // So that the first `status` already says whether stores can be maintained (ticket 11).
        git::refresh(&starting);
    })
    .await
    .map_err(io::Error::other)?;
    // Every file is read again whenever a request needs it; the tick is what makes a reload happen
    // when nothing asks, so that what follows from one — a Root queued for a full comparison, a
    // `Degraded` state — does not wait for a client.
    let ticker = tokio::spawn(configuration::reload_continually(state.clone()));
    let housekeeping = tokio::spawn(housekeeping::run_continually(state.clone()));
    let loopback = match &*state.listener {
        loopback::Listener::Listening(gate) => Some(gate.address()),
        _ => None,
    };
    let task = tokio::spawn(async move {
        // Held for as long as anything below runs, and released only once it has all stopped.
        let _pid_lock = pid_lock;
        let events = Arc::clone(&state.events);
        let housekeeping_state = Arc::clone(&state.housekeeping);
        let (signal, stopping) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            let _ = stop.await;
            // No pass and no job starts from here on; one that is waiting for its store ends failed, and
            // says so while the stream is still open.
            housekeeping_state.stop();
            // Said before the streams end, so that a tray tells a stop from a death (ticket 67).
            events.publish_now(keepster_api::Event::DaemonStopping);
            // An open event stream would otherwise hold the graceful stop forever.
            events.close();
            let _ = signal.send(true);
        });
        let until_stopped = |mut stopping: tokio::sync::watch::Receiver<bool>| async move {
            let _ = stopping.wait_for(|stopped| *stopped).await;
        };
        let browser = match (&*state.listener, tcp) {
            (loopback::Listener::Listening(gate), Some(tcp)) => Some(tokio::spawn(serve(
                tcp,
                loopback_router(state.clone(), Arc::clone(gate)),
                until_stopped(stopping.clone()),
            ))),
            _ => None,
        };
        let served = serve(socket, router(state.clone()), until_stopped(stopping)).await;
        if let Some(browser) = browser {
            match browser.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(%error, "the loopback listener stopped with an error")
                }
                Err(error) => tracing::warn!(%error, "the loopback listener's task failed"),
            }
        }
        if let loopback::Listener::Listening(gate) = &*state.listener {
            gate.withdraw();
        }
        // No further Trigger is accepted; the captures in flight finish and every pending debounce
        // window is captured before the daemon is done (ticket 05).
        ticker.abort();
        // A pass in flight holds a store's lock, which the last captures below need: it finishes first.
        if tokio::time::timeout(HOUSEKEEPING_DRAIN, housekeeping)
            .await
            .is_err()
        {
            tracing::warn!(
                "a housekeeping pass is still running; the daemon stops without waiting for it"
            );
        }
        state.runtimes.stop().await;
        served
    });

    Ok(RunningDaemon {
        control_path: config.control_path,
        loopback,
        shutdown: Some(shutdown),
        task,
    })
}

/// Bind the loopback listener on `port` — `0` for any free one — and mint the token guarding it.
///
/// At the start or not at all: a listener appearing while the daemon runs would be a second lifecycle,
/// with a port collision surfacing at a request rather than here (ticket 24).
async fn bind_loopback(
    port: u16,
    allowed_origins: &[String],
    token_path: PathBuf,
) -> io::Result<(Arc<loopback::Gate>, tokio::net::TcpListener)> {
    let tcp = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let gate = loopback::Gate::new(tcp.local_addr()?, allowed_origins, token_path)?;

    Ok((Arc::new(gate), tcp))
}

/// Take the pid file's lock, or say which daemon holds it.
///
/// A lock just released can still read as held for a moment: a process this one spawns holds a copy of
/// every descriptor between its fork and its exec, the lock's included — and a daemon spawns `git`, as
/// a test process spawns everything. A held lock is therefore asked again for a little while before it
/// is believed; a daemon that is really running holds it throughout, and is refused a second later.
async fn lock_pid_file(path: &Path) -> Result<keepster_service::PidLock, StartError> {
    const ATTEMPTS: u32 = 20;
    const PAUSE: std::time::Duration = std::time::Duration::from_millis(50);

    let mut attempt = 1;
    loop {
        match keepster_service::lock_pid_file(path) {
            Ok(lock) => return Ok(lock),
            Err(keepster_service::PidLockError::Held { pid }) if attempt >= ATTEMPTS => {
                return Err(StartError::AlreadyRunning { pid });
            }
            Err(keepster_service::PidLockError::Held { .. }) => {
                attempt += 1;
                tokio::time::sleep(PAUSE).await;
            }
            Err(keepster_service::PidLockError::Io(error)) => return Err(error.into()),
        }
    }
}

/// Why a daemon did not start.
#[derive(Debug)]
pub enum StartError {
    /// Another daemon holds the pid file's lock — over the same runtime directory, so the same user.
    /// `pid` is what it wrote there, if it had yet.
    AlreadyRunning {
        /// The live daemon's process id.
        pid: Option<u32>,
    },
    /// Anything else that kept it from binding or reading its state.
    Io(io::Error),
}

impl From<io::Error> for StartError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl std::fmt::Display for StartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning { pid: Some(pid) } => {
                write!(
                    formatter,
                    "a daemon is already running for this user (pid {pid})"
                )
            }
            Self::AlreadyRunning { pid: None } => {
                write!(formatter, "a daemon is already running for this user")
            }
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for StartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AlreadyRunning { .. } => None,
            Self::Io(error) => Some(error),
        }
    }
}

/// How long a stopping daemon waits for a Retention or Maintenance Pass in flight, inside the service
/// manager's stop timeout alongside the runtimes' own drain (ticket 05).
const HOUSEKEEPING_DRAIN: std::time::Duration = std::time::Duration::from_secs(2);

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
        store_format: Some(StoreFormat {
            container: keepster_store::CONTAINER_VERSION,
            content: keepster_core::ContentFormat::CURRENT.number(),
            sidecar: keepster_core::SIDECAR_VERSION,
            index: keepster_store::INDEX_SCHEMA_VERSION,
        }),
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
    /// Which Roots owe a Retention or Maintenance Pass.
    housekeeping: Arc<housekeeping::Housekeeping>,
    /// The `git` maintenance runs with, once it has been looked for.
    git: Arc<Mutex<Option<git::Found>>>,
    /// The jobs the daemon remembers (ticket 54).
    jobs: Arc<jobs::Jobs>,
    /// What happened, for every open event stream and the ones that reconnect.
    events: Arc<events::EventBus>,
    /// What the event stream last said about each Root's state and configuration.
    announced: Arc<Mutex<events::Announced>>,
    /// Which volume a path is on (ticket 56).
    volumes: Arc<dyn Volumes>,
    /// What the last capture of each Root found of its volume and its store.
    outcomes: Arc<volumes::Outcomes>,
    /// Whether a Capture Preview is walking: one runs at a time (ticket 57).
    previewing: Arc<std::sync::atomic::AtomicBool>,
    /// Where every Root's directory is: its record and its History Store.
    store_base: PathBuf,
    /// The loopback listener, or why there is none (ticket 60).
    listener: Arc<loopback::Listener>,
    /// What the platform last said about autostart and linger (ticket 61).
    service: Arc<autostart::Readings>,
    /// The level the log is written at, and a raise while it lasts (ticket 62).
    log: Arc<logging::LogLevels>,
    /// The web UI's preferences file; the lock makes writes one at a time (ticket 66).
    preferences: Arc<Mutex<keepster_store::PreferencesStore>>,
    /// What stopped the insurance, and the mute (ticket 67).
    alarms: Arc<alarms::Alarms>,
    /// What each attached tray last reported it can do (ticket 67).
    trays: Arc<Mutex<trays::Trays>>,
}

impl DaemonState {
    #[allow(
        clippy::too_many_arguments,
        reason = "one parameter per independent part of the state, each named at the one call"
    )]
    fn new(
        registry: Registry,
        config: keepster_app::config::Configuration,
        listener: loopback::Listener,
        log: Arc<logging::LogLevels>,
        events: Arc<events::EventBus>,
        store_base: PathBuf,
        watch_ceiling: Option<usize>,
        retry_after: std::time::Duration,
        housekeeping: HousekeepingTiming,
        volumes: Arc<dyn Volumes>,
    ) -> Self {
        Self {
            started: Instant::now(),
            registry: Arc::new(Mutex::new(registry)),
            store_locks: Arc::new(Mutex::new(HashMap::new())),
            config: Arc::new(Mutex::new(config)),
            runtimes: Arc::new(triggers::Runtimes::new(watch_ceiling, retry_after)),
            housekeeping: Arc::new(housekeeping::Housekeeping::new(housekeeping)),
            git: Arc::new(Mutex::new(None)),
            jobs: Arc::new(jobs::Jobs::default()),
            events,
            announced: Arc::new(Mutex::new(events::Announced::default())),
            volumes,
            outcomes: Arc::new(volumes::Outcomes::default()),
            previewing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            preferences: Arc::new(Mutex::new(keepster_store::PreferencesStore::at(
                &store_base,
            ))),
            store_base,
            listener: Arc::new(listener),
            service: Arc::new(autostart::Readings::default()),
            log,
            alarms: Arc::new(alarms::Alarms::default()),
            trays: Arc::new(Mutex::new(trays::Trays::default())),
        }
    }

    /// Tell every open event stream that `event` happened.
    fn publish(&self, event: keepster_api::Event) {
        self.events.publish_now(event);
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
    /// deadlock. The whole order is: a store's lock, the Registry, the configuration, the runtimes,
    /// housekeeping's table of what each Root owes, the `git` found, then what the event stream announced,
    /// the alarms and the event buffer. The jobs' table is innermost: taken with at most a store's lock held. A panic while it is held loses nothing that matters: every file is compared with
    /// what was read last, so the next resolution reads again whatever a panic left half-done.
    fn config(&self) -> MutexGuard<'_, keepster_app::config::Configuration> {
        self.config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What the daemon knows of its `git`: taken with at most a store's lock held, and never while it
    /// runs one.
    fn git(&self) -> MutexGuard<'_, Option<git::Found>> {
        self.git
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

    /// The trays' reports: a leaf, taken with nothing else held.
    fn trays(&self) -> MutexGuard<'_, trays::Trays> {
        self.trays
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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

    router
        .with_state(state)
        .layer(axum::middleware::from_fn(answered))
}

/// One debug line per request answered, on either listener: what a raised level is for.
async fn answered(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let response = next.run(request).await;
    tracing::debug!(%method, %path, status = response.status().as_u16(), "answered");

    response
}

/// What the loopback listener serves: the same `/v1` — the SPA gets no privileged endpoint — behind the
/// gate, and the page a browser session lands on.
fn loopback_router(state: DaemonState, gate: Arc<loopback::Gate>) -> Router {
    let (router, _) = on_both_listeners().split_for_parts();

    with_web_ui(router)
        .with_state(state)
        .layer(axum::middleware::from_fn(answered))
        .layer(axum::middleware::from_fn_with_state(gate, loopback::guard))
}

/// The embedded web UI, at every path no route of `/v1` takes.
#[cfg(feature = "embed-ui")]
fn with_web_ui(router: Router<DaemonState>) -> Router<DaemonState> {
    router.fallback(web::serve)
}

/// A build without the web UI (`embed-ui` off) lands a browser session on a placeholder.
#[cfg(not(feature = "embed-ui"))]
fn with_web_ui(router: Router<DaemonState>) -> Router<DaemonState> {
    router.route("/", axum::routing::get(landing))
}

/// Where a browser session lands in a build without the web UI: it still serves `/v1` here, because the
/// API is public for third-party clients (ticket 24).
#[cfg(not(feature = "embed-ui"))]
async fn landing() -> axum::response::Html<&'static str> {
    axum::response::Html(
        "<!doctype html><meta charset=\"utf-8\"><title>Keepster</title>\
         <p>Keepster's daemon is listening. This build carries no web UI; its API is served under \
         <code>/v1</code>.</p>",
    )
}

/// Every route, each with its description.
///
/// `routes!` takes the path from the handler's own `#[utoipa::path]` and hands the same string to axum,
/// so the route and the documented path are one literal: renaming a handler fails to compile, and a
/// handler left out is dead code. Handlers are grouped by path, because one `routes!` registers one
/// path.
fn api() -> OpenApiRouter<DaemonState> {
    on_both_listeners()
        .routes(routes!(diagnostics::diagnostics))
        .routes(routes!(ui::bootstrap))
        .routes(routes!(ui::rotate))
}

/// Every route both listeners serve, by the same `routes!` contract as [`api`]: all of `/v1` but the
/// browser's way in, which the control channel alone hands out.
fn on_both_listeners() -> OpenApiRouter<DaemonState> {
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
        .routes(routes!(pins::list))
        .routes(routes!(pins::pin, pins::unpin))
        .routes(routes!(jobs::maintain))
        .routes(routes!(jobs::forget))
        .routes(routes!(jobs::show))
        .routes(routes!(diff::diff))
        .routes(routes!(restore::restore))
        .routes(routes!(export::export))
        .routes(routes!(export::stat))
        .routes(routes!(export::archive))
        .routes(routes!(directories::list))
        .routes(routes!(search::paths))
        .routes(routes!(search::content))
        .routes(routes!(explain::explain))
        .routes(routes!(preferences::show, preferences::replace))
        .routes(routes!(preview::preview))
        .routes(routes!(configuration::show))
        .routes(routes!(logging::raise, logging::end))
        .routes(routes!(alarms::mute, alarms::unmute))
        .routes(routes!(trays::report, trays::withdraw))
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
    status_of(&state).await.map(Json)
}

/// The whole health surface as it stands: what `GET /v1/status` serves, and what a diagnostics bundle
/// carries verbatim.
async fn status_of(state: &DaemonState) -> Result<Status, problem::ProblemResponse> {
    let registered = roots::reported(state, &state.registry())?;
    let (maintenance, git) = git::report(state);
    let service = state.service.rows().await;

    Ok(Status {
        daemon: DaemonStatus {
            version: version_info(),
            pid: std::process::id(),
            uptime_seconds: state.started.elapsed().as_secs(),
            autostart: service.autostart,
            autostart_reason: service.autostart_reason,
            linger: service.linger,
            linger_reason: service.linger_reason,
            maintenance,
            git,
            listener: Some(state.listener.status()),
            log: Some(state.log.status()),
            alarms: Some(state.alarms.active()),
            muted_until: state
                .alarms
                .muted_until(roots::now())
                .and_then(roots::rfc3339),
            trays: Some(state.trays().current(Instant::now())),
        },
        roots: registered,
    })
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
