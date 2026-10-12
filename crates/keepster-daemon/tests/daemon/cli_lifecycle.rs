//! The shipped binary as a service manager runs it (ticket 61): `keepster daemon` with every per-user
//! directory pointed into a sandbox, stopped by a signal as systemd and launchd stop it.
//!
//! Unix only: these send signals, and on Windows the control channel is a pipe named after the user's
//! SID, which no environment variable moves into a sandbox.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use keepster_cli::DaemonClient;
use serde_json::Value;
use tempfile::TempDir;

const KEEPSTER: &str = env!("CARGO_BIN_EXE_keepster");

/// Long enough for a loaded CI runner; each wait ends as soon as what it waits for happens.
const PATIENCE: Duration = Duration::from_secs(30);

/// One user's worth of per-user directories, none of them the machine's own.
pub(crate) struct Sandbox {
    directory: TempDir,
}

impl Sandbox {
    pub(crate) fn new() -> Self {
        let directory = TempDir::new().expect("a temporary directory");
        for name in ["r", "data", "config", "home"] {
            std::fs::create_dir_all(directory.path().join(name)).expect("a directory");
        }

        Self { directory }
    }

    fn base(&self) -> PathBuf {
        self.directory
            .path()
            .canonicalize()
            .expect("the sandbox resolves")
    }

    /// Where a daemon run in this sandbox listens: the platform's own default, inside the sandbox.
    pub(crate) fn control_path(&self) -> PathBuf {
        self.base().join("r/keepster/daemon.sock")
    }

    /// The global configuration a daemon run in this sandbox reads, on Linux and on macOS alike.
    pub(crate) fn config_path(&self) -> PathBuf {
        if cfg!(target_os = "macos") {
            self.base()
                .join("home/Library/Application Support/Keepster/config.toml")
        } else {
            self.base().join("config/keepster/config.toml")
        }
    }

    /// Where the systemd user unit of this sandbox's user goes.
    pub(crate) fn unit_dir(&self) -> PathBuf {
        self.base().join("config/systemd/user")
    }

    pub(crate) fn write_config(&self, content: &[u8]) {
        let path = self.config_path();
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("its directory");
        std::fs::write(path, content).expect("the configuration");
    }

    /// A directory to watch.
    pub(crate) fn scratch(&self, name: &str) -> PathBuf {
        let path = self.base().join(name);
        std::fs::create_dir_all(&path).expect("a scratch directory");
        path
    }

    /// `keepster` with this sandbox's environment, no `--control-path`, and no session bus to reach —
    /// so a service manager it asks is the sandbox's, which does not exist.
    pub(crate) fn command(&self, args: &[&str]) -> tokio::process::Command {
        let base = self.base();
        let mut command = tokio::process::Command::new(KEEPSTER);
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", base.join("r"))
            .env("TMPDIR", base.join("r"))
            .env("XDG_DATA_HOME", base.join("data"))
            .env("XDG_CONFIG_HOME", base.join("config"))
            .env("HOME", base.join("home"))
            .env("USER", "keepster-sandbox-user")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// Run `keepster` to completion in this sandbox.
    pub(crate) async fn run(&self, args: &[&str]) -> std::process::Output {
        self.command(args)
            .output()
            .await
            .unwrap_or_else(|error| panic!("running keepster {args:?}: {error}"))
    }

    /// Start `keepster daemon` as a service manager would, and wait until it answers.
    pub(crate) async fn daemon(&self) -> tokio::process::Child {
        let child = self
            .command(&["daemon"])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the daemon starts");
        let client = DaemonClient::new(self.control_path());
        eventually("the daemon to answer", || async {
            client.get("/v1/version").await.ok().map(|_| ())
        })
        .await;

        child
    }

    pub(crate) fn client(&self) -> DaemonClient {
        DaemonClient::new(self.control_path())
    }
}

pub(crate) async fn eventually<T, F, Fut>(what: &str, mut probe: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(found) = probe().await {
            return found;
        }
        assert!(Instant::now() < deadline, "gave up waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn json(client: &DaemonClient, path: &str) -> Value {
    let response = client.get(path).await.expect("the daemon answers");
    assert!(
        response.status.is_success(),
        "GET {path}: {}",
        response.status
    );
    serde_json::from_slice(&response.body).expect("JSON")
}

async fn post(client: &DaemonClient, path: &str, body: Value) -> Value {
    let response = client
        .post(path, &serde_json::to_vec(&body).expect("a body"))
        .await
        .expect("the daemon answers");
    assert!(
        response.status.is_success(),
        "POST {path}: {}",
        response.status
    );
    serde_json::from_slice(&response.body).expect("JSON")
}

/// Send `signal` (`TERM`, `KILL`) to the process `pid`, as a service manager does.
fn signal(pid: u32, signal: &str) {
    let sent = std::process::Command::new("kill")
        .args([format!("-{signal}"), pid.to_string()])
        .status()
        .expect("kill runs");
    assert!(sent.success(), "kill -{signal} {pid}");
}

async fn register(sandbox: &Sandbox, project: &Path) -> String {
    let root = post(
        &sandbox.client(),
        "/v1/roots",
        serde_json::json!({ "path": project }),
    )
    .await;
    root["id"].as_str().expect("an id").to_owned()
}

#[tokio::test]
async fn a_second_keepster_daemon_exits_non_zero_naming_the_live_one() {
    let sandbox = Sandbox::new();
    let first = sandbox.daemon().await;
    let pid = first.id().expect("running");

    let second = sandbox.run(&["daemon"]).await;

    assert_eq!(
        second.status.code(),
        Some(5),
        "the code the unit does not restart on"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains(&format!("pid {pid}")), "{stderr}");
    let status = sandbox
        .client()
        .get("/v1/status")
        .await
        .expect("still served");
    assert_eq!(status.status, 200);
}

#[tokio::test]
async fn a_terminated_daemon_flushes_what_is_pending_before_it_exits() {
    // systemd and launchd stop a service with SIGTERM; a window that would not close for an hour is
    // captured by the stop or not at all.
    let sandbox = Sandbox::new();
    sandbox.write_config(
        b"[triggers]\ndebounce = \"1h\"\nrate_limit = \"1s\"\n[reconcile]\nat_start = false\n",
    );
    let mut daemon = sandbox.daemon().await;
    let project = sandbox.scratch("project");
    let root = register(&sandbox, &project).await;
    let client = sandbox.client();
    eventually("the registration's first Snapshot", || async {
        let timeline = json(&client, &format!("/v1/roots/{root}/snapshots")).await;
        (!timeline["snapshots"].as_array()?.is_empty()).then_some(())
    })
    .await;
    std::fs::write(project.join("unsaved-work.txt"), "the most recent edit").expect("a file");
    eventually("the edit to be pending", || async {
        let shown = json(&client, &format!("/v1/roots/{root}")).await;
        (shown["pending_paths"].as_u64() > Some(0)).then_some(())
    })
    .await;

    signal(daemon.id().expect("running"), "TERM");
    let exited = daemon.wait().await.expect("it exits");

    assert!(exited.success(), "a clean stop: {exited:?}");
    let _restarted = sandbox.daemon().await;
    let timeline = json(
        &client,
        &format!("/v1/roots/{root}/snapshots?path=unsaved-work.txt"),
    )
    .await;
    assert_eq!(
        timeline["snapshots"][0]["trigger"], "watcher",
        "the pending edit was captured on the way out: {timeline}"
    );
}

#[tokio::test]
async fn a_daemon_killed_outright_leaves_a_store_the_next_one_serves() {
    // Past the stop timeout a service manager kills; nothing it interrupts may cost the history.
    let sandbox = Sandbox::new();
    sandbox.write_config(keepster_testkit::MANUAL);
    let mut daemon = sandbox.daemon().await;
    let project = sandbox.scratch("project");
    std::fs::write(project.join("notes.md"), "first").expect("a file");
    let root = register(&sandbox, &project).await;
    let client = sandbox.client();
    post(
        &client,
        &format!("/v1/roots/{root}/snapshots"),
        serde_json::json!({}),
    )
    .await;

    signal(daemon.id().expect("running"), "KILL");
    daemon.wait().await.expect("it is gone");
    let _restarted = sandbox.daemon().await;

    let timeline = json(&client, &format!("/v1/roots/{root}/snapshots")).await;
    assert_eq!(
        timeline["snapshots"].as_array().map(Vec::len),
        Some(1),
        "{timeline}"
    );
    std::fs::write(project.join("notes.md"), "second").expect("an edit");
    post(
        &client,
        &format!("/v1/roots/{root}/snapshots"),
        serde_json::json!({}),
    )
    .await;
    let timeline = json(&client, &format!("/v1/roots/{root}/snapshots")).await;
    assert_eq!(
        timeline["snapshots"].as_array().map(Vec::len),
        Some(2),
        "{timeline}"
    );
}
