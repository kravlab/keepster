//! `keepster uninstall`'s erasing flags (ticket 68): erasing is only ever explicit, consent is given
//! separately from what is selected, and it happens with no daemon running. Run in the sandbox of
//! `cli_lifecycle`, whose service manager cannot be reached, so the autostart half is a no-op.
#![cfg(unix)]

use std::path::{Path, PathBuf};

use crate::cli_lifecycle::Sandbox;
use crate::cli_service::text;

/// One Root's own directory under the store base, as the Registry lays it out.
struct Stored {
    dir: PathBuf,
}

impl Stored {
    /// Whether its record says it is watched.
    fn watched(&self) -> bool {
        self.dir.join("root.toml").is_file()
    }

    /// Whether its record says it is no longer watched, its history kept.
    fn unwatched(&self) -> bool {
        self.dir.join("unwatched.toml").is_file()
    }

    /// Whether its History Store is there.
    fn history(&self) -> bool {
        self.dir.join("history.git").exists()
    }

    /// Whether its derived index is there.
    fn index(&self) -> bool {
        self.dir.join("index.sqlite").exists()
    }
}

/// Watch two directories and capture each once, then stop the daemon, as `keepster uninstall` finds a
/// machine after its unit stopped the daemon: the first Root watched, the second unwatched with its
/// history kept.
async fn two_roots_with_history(sandbox: &Sandbox) -> (Stored, Stored) {
    let mut daemon = sandbox.daemon().await;
    let mut stored = Vec::new();
    for name in ["kept", "dropped"] {
        let project = sandbox.scratch(name);
        std::fs::write(project.join("notes.txt"), name).expect("a file");
        let watched = sandbox.run(&["watch", path(&project)]).await;
        assert_eq!(watched.status.code(), Some(0), "{}", text(&watched.stderr));
        let captured = sandbox.run(&["snapshot", "-r", path(&project)]).await;
        assert_eq!(
            captured.status.code(),
            Some(0),
            "{}",
            text(&captured.stderr)
        );
        stored.push(project);
    }
    let unwatched = sandbox.run(&["unwatch", "-r", path(&stored[1])]).await;
    assert_eq!(
        unwatched.status.code(),
        Some(0),
        "{}",
        text(&unwatched.stderr)
    );
    daemon.kill().await.expect("the daemon stops");

    let mut dirs: Vec<PathBuf> = std::fs::read_dir(sandbox.store_base())
        .expect("the store base")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 2, "one directory per Root: {dirs:?}");
    // Identities are time-ordered, so the first registered sorts first.
    let dropped = Stored {
        dir: dirs.pop().expect("the second"),
    };
    let kept = Stored {
        dir: dirs.pop().expect("the first"),
    };
    assert!(kept.watched() && kept.history() && kept.index());
    assert!(dropped.unwatched() && dropped.history());

    (kept, dropped)
}

/// A sandbox path as a command-line argument.
fn path(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 sandbox")
}

#[tokio::test]
async fn an_erasing_flag_without_consent_is_refused_and_nothing_is_touched() {
    let sandbox = Sandbox::new();
    let (kept, dropped) = two_roots_with_history(&sandbox).await;
    let entry = sandbox.tray_entry();
    std::fs::create_dir_all(entry.parent().expect("a parent")).expect("its directory");
    std::fs::write(&entry, b"[Desktop Entry]\n").expect("the tray's entry");

    for flag in ["--history", "--roots", "--purge"] {
        let run = sandbox.run(&["uninstall", flag]).await;

        assert_eq!(run.status.code(), Some(2), "{flag}: {}", text(&run.stderr));
        assert!(
            text(&run.stderr).contains("--yes"),
            "{flag}: {}",
            text(&run.stderr)
        );
    }
    assert!(kept.watched() && kept.history());
    assert!(dropped.unwatched() && dropped.history());
    assert!(entry.exists(), "a refused uninstall removes nothing at all");
}

#[tokio::test]
async fn uninstall_alone_erases_nothing() {
    let sandbox = Sandbox::new();
    let (kept, dropped) = two_roots_with_history(&sandbox).await;

    let run = sandbox.run(&["uninstall"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(kept.watched() && kept.history() && kept.index());
    assert!(dropped.unwatched() && dropped.history());
}

#[tokio::test]
async fn history_erases_every_roots_history_and_keeps_the_watched_ones_registered() {
    let sandbox = Sandbox::new();
    let (kept, dropped) = two_roots_with_history(&sandbox).await;

    let run = sandbox.run(&["uninstall", "--history", "--yes"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(
        kept.watched(),
        "still registered, to start its history over"
    );
    assert!(!kept.history() && !kept.index());
    assert!(
        !dropped.dir.exists(),
        "a Root no longer watched held nothing but its history"
    );
}

#[tokio::test]
async fn roots_unregisters_every_root_and_keeps_its_history() {
    let sandbox = Sandbox::new();
    let (kept, dropped) = two_roots_with_history(&sandbox).await;

    let run = sandbox.run(&["uninstall", "--roots", "--yes"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(
        !kept.watched() && kept.unwatched(),
        "unwatched, as `unwatch` leaves it"
    );
    assert!(kept.history(), "left to decay, its last state pinned");
    assert!(dropped.unwatched() && dropped.history());
}

#[tokio::test]
async fn history_and_roots_together_erase_every_root() {
    let sandbox = Sandbox::new();
    let (kept, dropped) = two_roots_with_history(&sandbox).await;

    let run = sandbox
        .run(&["uninstall", "--history", "--roots", "--yes"])
        .await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(!kept.dir.exists() && !dropped.dir.exists());
}

#[tokio::test]
async fn purge_erases_everything_keepster_wrote_for_this_user() {
    let sandbox = Sandbox::new();
    two_roots_with_history(&sandbox).await;
    sandbox.write_config(b"[files]\nexclude = [\"*.log\"]\n");
    assert!(sandbox.log_file().exists(), "the daemon logged");

    let run = sandbox.run(&["uninstall", "--purge", "--yes"]).await;

    assert_eq!(run.status.code(), Some(0), "{}", text(&run.stderr));
    assert!(
        !sandbox.store_base().exists(),
        "the Registry and every store"
    );
    assert!(!sandbox.config_path().exists(), "the configuration");
    assert!(!sandbox.log_file().exists(), "the logs");
}

#[tokio::test]
async fn nothing_is_erased_while_a_daemon_still_runs() {
    // One no service manager started, so no unit removal stopped it: it may be writing a store now.
    let sandbox = Sandbox::new();
    let (kept, _dropped) = two_roots_with_history(&sandbox).await;
    let _bare = sandbox.daemon().await;

    let run = sandbox.run(&["uninstall", "--history", "--yes"]).await;

    assert_eq!(run.status.code(), Some(5), "{}", text(&run.stderr));
    assert!(
        text(&run.stderr).contains("still running"),
        "{}",
        text(&run.stderr)
    );
    assert!(kept.history(), "nothing was erased");
}

#[tokio::test]
async fn nothing_is_erased_while_a_daemon_holds_its_lock_without_answering() {
    // A daemon past closing its socket may still be writing a store: its lock, not its socket, says
    // whether it is gone.
    let sandbox = Sandbox::new();
    let (kept, _dropped) = two_roots_with_history(&sandbox).await;
    let runtime = sandbox
        .control_path()
        .parent()
        .expect("the runtime directory")
        .to_path_buf();
    let lock = keepster_service::lock_pid_file(&runtime.join(keepster_service::PID_FILE))
        .expect("the daemon's lock");

    let run = sandbox.run(&["uninstall", "--history", "--yes"]).await;

    assert_eq!(run.status.code(), Some(5), "{}", text(&run.stderr));
    assert!(kept.history(), "nothing was erased");
    drop(lock);
}
