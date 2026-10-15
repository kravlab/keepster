//! The test harness: a real daemon, over a real socket, driven by the real client.
//!
//! This is the highest seam in the workspace and the one most tests belong at (ticket 31). The daemon
//! under test is the daemon that ships, and the client is the CLI's own; the one stand-in is which
//! volume a path is on, [`FakeVolumes`], since no test can unplug a disk from the machine it runs on.
//! Dev-only, never published, and allowed to depend on anything.
//!
//! Beside the harness sits [`bulk`], the seeded generator whose frozen profiles the performance
//! tripwire is calibrated to (ticket 30); it shares nothing with correctness fixtures, whose cases must
//! be free to grow (ticket 31). [`thresholds`] reads the limits that tripwire is held to and the profile
//! digests they were calibrated against, [`environment`] describes the machine a run measures on, and
//! [`verdict`] and [`report`] judge a run's measurements and write them down. [`lines`] names what the
//! tripwire measures and why, and [`tripwire`] runs it. Apart from all of it, [`degenerate`] catalogues
//! the small edge-case trees ordinary fixtures never hold, [`platform`] records what each came to on
//! each platform, and [`host`] names the machine any report was made on, while [`skips`] finds a test
//! that prints a skip instead (ticket 70). [`conformance`] records what the operating system's watcher
//! delivers for each documented class of change, and [`coverage`] which crate no test reaches, both
//! gating nothing.

pub mod bulk;
pub mod conformance;
pub mod coverage;
pub mod degenerate;
pub mod environment;
pub mod host;
pub mod lines;
mod loopback;
pub mod platform;
pub mod report;
pub mod skips;
pub mod thresholds;
pub mod tripwire;
pub mod verdict;
mod volumes;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use volumes::FakeVolumes;

use keepster_cli::{DaemonClient, EventStream, Response};
use keepster_daemon::{DaemonConfig, RunningDaemon};
use tempfile::TempDir;

/// The identity of the volume every path a harness hands out is on, unless a test mounts another.
pub const HARNESS_VOLUME: &str = "uuid:harness";

/// A global configuration under which a Root is captured only when a test asks: no Trigger is active and
/// no Reconciliation runs at the start.
///
/// For tests about what one capture holds or how history reads back, where a Snapshot the daemon took
/// by itself would race the one the test took. Tests about automatic capture configure it themselves.
pub const MANUAL: &[u8] = b"[triggers]\nactive = []\n\n[reconcile]\nat_start = false\n";

/// What a CLI invocation produced.
///
/// `stdout` stays bytes so that a passthrough can be compared byte for byte; `stderr` is text because
/// it carries messages for a person.
#[derive(Debug)]
pub struct CliRun {
    /// The process exit code, or `None` when a signal ended it.
    pub exit_code: Option<i32>,
    /// Everything written to standard output.
    pub stdout: Vec<u8>,
    /// Everything written to standard error.
    pub stderr: String,
}

/// A running daemon plus a client for it, cleaned up on drop.
pub struct Harness {
    daemon: RunningDaemon,
    client: DaemonClient,
    /// The temporary directory as the operating system names it once every link is resolved. Every
    /// path the harness hands out starts here, so a Root registered from a test and the working
    /// directory a CLI reports name the same place: on macOS the temporary directory sits under
    /// `/var`, a link to `/private/var`, and a process started inside it reports the latter.
    base: PathBuf,
    /// What this daemon was started with beyond its directory, kept across a restart.
    options: Options,
    // Dropped last, after the daemon has been asked to stop and the socket file is gone.
    directory: TempDir,
}

/// A daemon that has stopped, with everything it wrote still on disk, waiting to be started again.
///
/// The window in between is the one a reboot leaves: whatever a test changes here, no watcher sees.
pub struct Stopped {
    options: Options,
    directory: TempDir,
}

impl Stopped {
    /// The control channel the next start will listen on, which nothing listens on now.
    pub fn control_path(&self) -> PathBuf {
        control_path(&resolved(self.directory.path()))
    }

    /// Start a fresh daemon over the same control channel and the same store.
    pub async fn start(self) -> Harness {
        Harness::start_in(self.directory, self.options).await
    }
}

/// How a test's daemon differs from the default one.
#[derive(Debug, Clone, Default)]
struct Options {
    /// The daemon-wide watch ceiling.
    watch_ceiling: Option<usize>,
    /// The first wait before an incomplete pass is retried.
    retry_after: Option<Duration>,
    /// How Retention and maintenance are paced.
    housekeeping: Option<keepster_daemon::HousekeepingTiming>,
    /// What is mounted where, kept across a restart as a machine's disks are.
    volumes: Arc<FakeVolumes>,
}

impl Harness {
    /// Start a daemon on a private, short-lived control channel with an empty store.
    ///
    /// The control path is kept short on purpose: `sun_path` holds 104 bytes on macOS and 108 on Linux,
    /// so a deep temporary directory would fail to bind on one platform and pass on another. The store
    /// base is temporary too, so no test can touch the machine's real Registry.
    pub async fn start() -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        Self::start_in(directory, Options::default()).await
    }

    /// Start a daemon whose Roots together may hold at most `ceiling` directory watches.
    pub async fn start_with_watch_ceiling(ceiling: usize) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let options = Options {
            watch_ceiling: Some(ceiling),
            ..Options::default()
        };
        Self::start_in(directory, options).await
    }

    /// Start a daemon that passes over a Root again `after` an incomplete pass, rather than the
    /// production half-minute, so a test can see the retry happen.
    pub async fn start_with_retry_after(after: Duration) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        let options = Options {
            retry_after: Some(after),
            ..Options::default()
        };
        Self::start_in(directory, options).await
    }

    /// Start a daemon whose global configuration already holds `content` and which paces Retention and
    /// maintenance by `timing`, so a test sees a pass in moments rather than minutes.
    pub async fn start_with_housekeeping(
        timing: keepster_daemon::HousekeepingTiming,
        content: &[u8],
    ) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        std::fs::write(resolved(directory.path()).join("config.toml"), content)
            .expect("the global configuration");
        let options = Options {
            housekeeping: Some(timing),
            ..Options::default()
        };

        Self::start_in(directory, options).await
    }

    /// Start a daemon whose global configuration already holds `content` when it first reads it.
    pub async fn start_with_global_config(content: &[u8]) -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        std::fs::write(resolved(directory.path()).join("config.toml"), content)
            .expect("the global configuration");

        Self::start_in(directory, Options::default()).await
    }

    /// Stop this daemon, keeping everything it wrote, so that a test can change the Working Tree while
    /// nothing watches it.
    pub async fn stop(self) -> Stopped {
        let Self {
            daemon,
            directory,
            options,
            ..
        } = self;

        daemon.shutdown().await.expect("the daemon stops cleanly");

        Stopped { options, directory }
    }

    /// Stop this daemon and start a fresh one over the same control channel and the same store.
    ///
    /// What survives a restart is exactly what was written to disk, which is how the Registry's
    /// durability is asserted without reaching into its files.
    pub async fn restart(self) -> Self {
        self.stop().await.start().await
    }

    async fn start_in(directory: TempDir, options: Options) -> Self {
        let base = resolved(directory.path());
        let control_path = control_path(&base);
        // Everything a test makes is on one volume the platform can name, as on most machines; a test
        // about volumes mounts others inside it.
        options.volumes.mount(&base, Some(HARNESS_VOLUME));

        let daemon = keepster_daemon::start(daemon_config(&base, &options))
            .await
            .expect("the daemon binds its control channel");

        Self {
            client: DaemonClient::new(control_path),
            daemon,
            base,
            options,
            directory,
        }
    }

    /// Start a second daemon over this one's runtime directory, control channel and store, as a second
    /// `keepster daemon` by the same user would — which the first one's lock is there to refuse.
    pub async fn start_another(
        &self,
    ) -> Result<keepster_daemon::RunningDaemon, keepster_daemon::StartError> {
        keepster_daemon::start(daemon_config(&self.base, &self.options)).await
    }

    /// What this daemon is told is mounted where, for a test to unmount and remount at will.
    pub fn volumes(&self) -> &FakeVolumes {
        &self.options.volumes
    }

    /// The control channel this daemon is listening on, for driving the real CLI against it.
    pub fn control_path(&self) -> &Path {
        self.daemon.control_path()
    }

    /// Where the loopback listener is, or `None` when it is off.
    pub fn loopback(&self) -> Option<std::net::SocketAddr> {
        self.daemon.loopback()
    }

    /// The file this daemon writes the loopback listener's token into, whether or not it did.
    pub fn token_path(&self) -> PathBuf {
        self.base.join(keepster_daemon::TOKEN_FILE)
    }

    /// Send one request to the loopback listener, as a browser or a third-party client would, with
    /// exactly the headers given — no `Host` is added, so a test decides what the daemon is told.
    ///
    /// Fails the test when the listener is off or cannot be reached.
    pub async fn loopback_request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> Response {
        let address = self.loopback().expect("the loopback listener is on");
        loopback::request(address, method, path, headers)
            .await
            .unwrap_or_else(|error| panic!("{method} {path} over loopback failed: {error}"))
    }

    /// `GET` the given path from the loopback listener, naming it in `Host` as a browser would and
    /// sending `headers` besides.
    pub async fn loopback_get(&self, path: &str, headers: &[(&str, &str)]) -> Response {
        let host = self
            .loopback()
            .expect("the loopback listener is on")
            .to_string();
        let mut all = vec![("host", host.as_str())];
        all.extend_from_slice(headers);

        self.loopback_request("GET", path, &all).await
    }

    /// The directory the daemon keeps one Root's record and History Store in.
    ///
    /// Tests use it to assert what deregistering does and does not erase — the one question where the
    /// answer is a file on disk rather than an answer from the API.
    pub fn store_dir(&self, root_id: &str) -> PathBuf {
        self.store_base().join(root_id)
    }

    /// The directory that holds every Root's directory, and beside them the daemon's own files such as
    /// the UI preferences.
    pub fn store_base(&self) -> PathBuf {
        self.base.join("store")
    }

    /// The global configuration file this daemon reads, which need not exist.
    pub fn config_path(&self) -> PathBuf {
        self.base.join("config.toml")
    }

    /// Write the global configuration this daemon reads, replacing whatever it held.
    ///
    /// The daemon reads it at every capture and every `explain`, so the next request sees it.
    ///
    /// Written as an editor saves, beside the file and renamed over it: a plain write truncates first,
    /// and a daemon reading in between would take the empty file, validly, for the configuration.
    pub fn write_global_config(&self, content: &[u8]) {
        let saving = self.base.join("config.toml.saving");
        std::fs::write(&saving, content).expect("the global configuration");
        std::fs::rename(saving, self.base.join("config.toml")).expect("saved");
    }

    /// Create a directory beside the daemon's own state, for use as a Watched Root.
    pub fn scratch(&self, name: &str) -> PathBuf {
        let path = self.base.join(name);
        std::fs::create_dir_all(&path).expect("a scratch directory");

        path
    }

    /// `GET` the given path, failing the test if the daemon could not be reached.
    pub async fn get(&self, path: &str) -> Response {
        self.client
            .get(path)
            .await
            .unwrap_or_else(|error| panic!("GET {path} failed: {error}"))
    }

    /// `GET` the given path with extra request headers, failing the test if the daemon could not be
    /// reached.
    pub async fn get_with(&self, path: &str, headers: &[(&str, &str)]) -> Response {
        self.client
            .get_with(path, headers)
            .await
            .unwrap_or_else(|error| panic!("GET {path} failed: {error}"))
    }

    /// Open the daemon's live event stream, resuming after `last_event_id` if given, failing the test if
    /// the daemon could not be reached or refused it.
    pub async fn events(&self, last_event_id: Option<&str>) -> EventStream {
        self.client
            .events(last_event_id)
            .await
            .unwrap_or_else(|error| panic!("GET /v1/events failed: {error}"))
    }

    /// `POST` JSON to the given path, failing the test if the daemon could not be reached.
    pub async fn post(&self, path: &str, body: &serde_json::Value) -> Response {
        self.client
            .post(path, &serde_json::to_vec(body).expect("a JSON body"))
            .await
            .unwrap_or_else(|error| panic!("POST {path} failed: {error}"))
    }

    /// `PATCH` JSON at the given path, failing the test if the daemon could not be reached.
    pub async fn patch(&self, path: &str, body: &serde_json::Value) -> Response {
        self.client
            .patch(path, &serde_json::to_vec(body).expect("a JSON body"))
            .await
            .unwrap_or_else(|error| panic!("PATCH {path} failed: {error}"))
    }

    /// `PUT` the given path, without a body, failing the test if the daemon could not be reached.
    pub async fn put(&self, path: &str) -> Response {
        self.client
            .put(path)
            .await
            .unwrap_or_else(|error| panic!("PUT {path} failed: {error}"))
    }

    /// `PUT` JSON at the given path, failing the test if the daemon could not be reached.
    pub async fn put_json(&self, path: &str, body: &serde_json::Value) -> Response {
        self.client
            .put_json(path, &serde_json::to_vec(body).expect("a JSON body"))
            .await
            .unwrap_or_else(|error| panic!("PUT {path} failed: {error}"))
    }

    /// `DELETE` the given path, failing the test if the daemon could not be reached.
    pub async fn delete(&self, path: &str) -> Response {
        self.client
            .delete(path)
            .await
            .unwrap_or_else(|error| panic!("DELETE {path} failed: {error}"))
    }

    /// Run the real CLI binary against this daemon.
    ///
    /// `binary` comes from the caller's `env!("CARGO_BIN_EXE_keepster")`, which only the crate that
    /// defines the binary can expand — so the harness takes it rather than guessing a target path.
    pub async fn run_cli(&self, binary: &str, args: &[&str]) -> CliRun {
        self.run_cli_with_control_path(binary, self.control_path(), args)
            .await
    }

    /// Start the real CLI binary against this daemon and leave it running, for a command that does not
    /// end by itself, such as `keepster log --follow`. Its standard output is piped for the test to read;
    /// it is killed when the handle is dropped.
    pub fn spawn_cli(&self, binary: &str, args: &[&str]) -> tokio::process::Child {
        tokio::process::Command::new(binary)
            .arg("--control-path")
            .arg(self.control_path())
            .args(args)
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|error| panic!("starting {binary}: {error}"))
    }

    /// Run the real CLI binary from inside `directory`, which is how the walk-up from a working
    /// directory is exercised.
    pub async fn run_cli_in(&self, directory: &Path, binary: &str, args: &[&str]) -> CliRun {
        self.run_cli_inner(binary, self.control_path(), Some(directory), args)
            .await
    }

    /// Run the real CLI binary from inside `directory` with arguments that need not be UTF-8 — a name
    /// on Unix is bytes, and the CLI must take it as it is (ticket 74).
    pub async fn run_cli_in_os(&self, directory: &Path, binary: &str, args: &[&OsStr]) -> CliRun {
        self.run_cli_os(binary, self.control_path(), Some(directory), args)
            .await
    }

    /// Run the real CLI binary against an arbitrary control channel, including one nothing is
    /// listening on.
    pub async fn run_cli_with_control_path(
        &self,
        binary: &str,
        control_path: &Path,
        args: &[&str],
    ) -> CliRun {
        self.run_cli_inner(binary, control_path, None, args).await
    }

    async fn run_cli_inner(
        &self,
        binary: &str,
        control_path: &Path,
        directory: Option<&Path>,
        args: &[&str],
    ) -> CliRun {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.run_cli_os(binary, control_path, directory, &args)
            .await
    }

    /// Run the real CLI binary, from `directory` when one is given, with `args` as they are.
    async fn run_cli_os(
        &self,
        binary: &str,
        control_path: &Path,
        directory: Option<&Path>,
        args: &[&OsStr],
    ) -> CliRun {
        let mut command = tokio::process::Command::new(binary);
        command.arg("--control-path").arg(control_path).args(args);
        if let Some(directory) = directory {
            command.current_dir(directory);
        }

        let output = command
            .output()
            .await
            .unwrap_or_else(|error| panic!("running {binary}: {error}"));

        CliRun {
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// `GET` the given path and parse the body as JSON, requiring success.
    pub async fn get_json(&self, path: &str) -> serde_json::Value {
        json_body("GET", path, self.get(path).await)
    }

    /// `POST` JSON to the given path and parse the answer, requiring success.
    pub async fn post_json(&self, path: &str, body: &serde_json::Value) -> serde_json::Value {
        json_body("POST", path, self.post(path, body).await)
    }
}

/// How a test's daemon over `base` is started.
fn daemon_config(base: &Path, options: &Options) -> DaemonConfig {
    DaemonConfig {
        control_path: control_path(base),
        runtime_dir: base.to_path_buf(),
        // Any free port: tests run in parallel, and a fixed one would be taken by the first.
        loopback_port: Some(0),
        store_base: base.join("store"),
        config_path: Some(base.join("config.toml")),
        watch_ceiling: options.watch_ceiling,
        retry_after: options.retry_after,
        housekeeping: options.housekeeping,
        volumes: Some(Arc::clone(&options.volumes) as Arc<dyn keepster_daemon::Volumes>),
        // The log is the process's, and a test process runs many daemons: none writes a file.
        log: None,
    }
}

/// `path` with every link resolved, where the platform's paths allow it.
///
/// On Windows, canonicalising yields a verbatim `\\?\` path that much of the toolchain does not accept,
/// and the temporary directory there is no link, so it is used as it is.
fn resolved(path: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        path.canonicalize()
            .expect("the temporary directory resolves")
    }

    #[cfg(not(unix))]
    {
        path.to_path_buf()
    }
}

/// The control channel for a daemon whose state lives in `base`.
///
/// A socket file on Unix, kept short because `sun_path` holds 104 bytes on macOS and 108 on Linux; a
/// named pipe on Windows, where a control channel is not a file and must be named `\\.\pipe\…`. The pipe
/// takes the temporary directory's own unique name, so parallel tests never share one, and a restart
/// over the same directory reuses it.
fn control_path(base: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let unique = base
            .file_name()
            .expect("a temporary directory has a name")
            .to_string_lossy();
        PathBuf::from(format!(r"\\.\pipe\keepster-test-{unique}"))
    }

    #[cfg(not(windows))]
    {
        base.join("d.sock")
    }
}

fn json_body(method: &str, path: &str, response: Response) -> serde_json::Value {
    assert!(
        response.status.is_success(),
        "{method} {path} returned {}: {}",
        response.status,
        String::from_utf8_lossy(&response.body)
    );

    serde_json::from_slice(&response.body)
        .unwrap_or_else(|error| panic!("{method} {path} returned invalid JSON: {error}"))
}
