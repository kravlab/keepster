//! The test harness: a real daemon, over a real socket, driven by the real client.
//!
//! This is the highest seam in the workspace and the one most tests belong at (ticket 31). There are
//! no test doubles here: the daemon under test is the daemon that ships, and the client is the CLI's
//! own. Dev-only, never published, and allowed to depend on anything.

use std::path::Path;

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
    _runtime_dir: TempDir,
}

impl Harness {
    /// Start a daemon on a private, short-lived control channel.
    ///
    /// The path is kept short on purpose: `sun_path` holds 104 bytes on macOS and 108 on Linux, so a
    /// deep temporary directory would fail to bind on one platform and pass on another.
    pub async fn start() -> Self {
        let runtime_dir = TempDir::new().expect("a temporary runtime directory");
        let control_path = runtime_dir.path().join("d.sock");

        let daemon = keepster_daemon::start(DaemonConfig {
            control_path: control_path.clone(),
        })
        .await
        .expect("the daemon binds its control channel");

        Self {
            client: DaemonClient::new(control_path),
            daemon,
            _runtime_dir: runtime_dir,
        }
    }

    /// The control channel this daemon is listening on, for driving the real CLI against it.
    pub fn control_path(&self) -> &Path {
        self.daemon.control_path()
    }

    /// `GET` the given path, failing the test if the daemon could not be reached.
    pub async fn get(&self, path: &str) -> Response {
        self.client
            .get(path)
            .await
            .unwrap_or_else(|error| panic!("GET {path} failed: {error}"))
    }

    /// Run the real CLI binary against this daemon.
    ///
    /// `binary` comes from the caller's `env!("CARGO_BIN_EXE_keepster")`, which only the crate that
    /// defines the binary can expand — so the harness takes it rather than guessing a target path.
    pub async fn run_cli(&self, binary: &str, args: &[&str]) -> CliRun {
        self.run_cli_with_control_path(binary, self.control_path(), args)
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
        let output = tokio::process::Command::new(binary)
            .arg("--control-path")
            .arg(control_path)
            .args(args)
            .output()
            .await
            .unwrap_or_else(|error| panic!("running {binary}: {error}"));

        CliRun {
            exit_code: output.status.code(),
            stdout: output.stdout,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// `GET` the given path and parse the body as JSON.
    pub async fn get_json(&self, path: &str) -> serde_json::Value {
        let response = self.get(path).await;
        assert!(
            response.status.is_success(),
            "GET {path} returned {}",
            response.status
        );
        serde_json::from_slice(&response.body)
            .unwrap_or_else(|error| panic!("GET {path} returned invalid JSON: {error}"))
    }
}
