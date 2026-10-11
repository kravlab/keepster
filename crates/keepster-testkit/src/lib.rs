//! The test harness: a real daemon, over a real socket, driven by the real client.
//!
//! This is the highest seam in the workspace and the one most tests belong at (ticket 31). There are
//! no test doubles here: the daemon under test is the daemon that ships, and the client is the CLI's
//! own. Dev-only, never published, and allowed to depend on anything.

use std::path::{Path, PathBuf};

use keepster_cli::{DaemonClient, Response};
use keepster_daemon::{DaemonConfig, RunningDaemon};
use tempfile::TempDir;

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
    // Dropped last, after the daemon has been asked to stop and the socket file is gone.
    directory: TempDir,
}

impl Harness {
    /// Start a daemon on a private, short-lived control channel with an empty store.
    ///
    /// The control path is kept short on purpose: `sun_path` holds 104 bytes on macOS and 108 on Linux,
    /// so a deep temporary directory would fail to bind on one platform and pass on another. The store
    /// base is temporary too, so no test can touch the machine's real Registry.
    pub async fn start() -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        Self::start_in(directory).await
    }

    /// Stop this daemon and start a fresh one over the same control channel and the same store.
    ///
    /// What survives a restart is exactly what was written to disk, which is how the Registry's
    /// durability is asserted without reaching into its files.
    pub async fn restart(self) -> Self {
        let Self {
            daemon, directory, ..
        } = self;

        daemon.shutdown().await.expect("the daemon stops cleanly");

        Self::start_in(directory).await
    }

    async fn start_in(directory: TempDir) -> Self {
        let control_path = directory.path().join("d.sock");
        let store_base = directory.path().join("store");
        let config_path = directory.path().join("config.toml");

        let daemon = keepster_daemon::start(DaemonConfig {
            control_path: control_path.clone(),
            store_base,
            config_path: Some(config_path),
        })
        .await
        .expect("the daemon binds its control channel");

        Self {
            client: DaemonClient::new(control_path),
            daemon,
            directory,
        }
    }

    /// The control channel this daemon is listening on, for driving the real CLI against it.
    pub fn control_path(&self) -> &Path {
        self.daemon.control_path()
    }

    /// The directory the daemon keeps one Root's record and History Store in.
    ///
    /// Tests use it to assert what deregistering does and does not erase — the one question where the
    /// answer is a file on disk rather than an answer from the API.
    pub fn store_dir(&self, root_id: &str) -> PathBuf {
        self.directory.path().join("store").join(root_id)
    }

    /// Write the global configuration this daemon reads, replacing whatever it held.
    ///
    /// The daemon reads it at every capture and every `explain`, so the next request sees it.
    pub fn write_global_config(&self, content: &[u8]) {
        std::fs::write(self.directory.path().join("config.toml"), content)
            .expect("the global configuration");
    }

    /// Create a directory beside the daemon's own state, for use as a Watched Root.
    pub fn scratch(&self, name: &str) -> PathBuf {
        let path = self.directory.path().join(name);
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

    /// Run the real CLI binary from inside `directory`, which is how the walk-up from a working
    /// directory is exercised.
    pub async fn run_cli_in(&self, directory: &Path, binary: &str, args: &[&str]) -> CliRun {
        self.run_cli_inner(binary, self.control_path(), Some(directory), args)
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
