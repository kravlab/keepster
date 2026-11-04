//! Ending the tray that holds the tray's lock (issue 49), as `keepster install` does before it starts the
//! tray beside the new `keepster`. The tray here is a shell standing in for it, which takes the lock and
//! writes its pid the way the tray does; no real tray, session or service manager is involved.
#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use keepster_service::autostart::tray::{TrayEnded, TrayNotEnded, end_running};
use keepster_service::{Pid, PidLockHolder, pid_lock_holder};

/// Long enough for a loaded CI runner; each wait ends as soon as what it waits for happens.
const PATIENCE: Duration = Duration::from_secs(30);

/// A process standing in for a running tray, killed and reaped however the test ends.
struct StandIn(Child);

impl StandIn {
    /// Start a shell that takes the lock on `lock` and writes its pid there, as the tray does, running
    /// `before` first — `trap '' TERM` makes one that will not end when asked — and wait until it holds
    /// the lock under its own pid.
    fn holding(lock: &Path, before: &str) -> Self {
        let stand_in = Self::locking(lock, before, "");
        stand_in.wait_for(
            lock,
            PidLockHolder::Held {
                pid: Some(stand_in.pid()),
            },
        );

        stand_in
    }

    /// Start a shell that takes the lock on `lock`, runs `between` — `sleep 0.5` makes one slow to write
    /// its pid — and only then writes its pid there, and wait until it holds the lock with no pid
    /// written yet.
    fn holding_unnamed(lock: &Path, between: &str) -> Self {
        let stand_in = Self::locking(lock, "", between);
        stand_in.wait_for(lock, PidLockHolder::Held { pid: None });

        stand_in
    }

    /// Start a shell that runs `before`, takes the lock on `lock`, runs `between`, writes its pid there
    /// and sleeps.
    fn locking(lock: &Path, before: &str, between: &str) -> Self {
        let script = format!(
            "{before}\nexec 9<>\"$1\"\nflock -n 9 || exit 1\n{between}\nprintf %s $$ > \"$1\"\n\
             exec sleep 60\n"
        );
        let child = Command::new("sh")
            .args(["-c", &script, "sh"])
            .arg(lock)
            .stdin(Stdio::null())
            .spawn()
            .expect("the stand-in starts");

        Self(child)
    }

    /// Wait until `pid_lock_holder` reads `holder` from `lock`.
    fn wait_for(&self, lock: &Path, holder: PidLockHolder) {
        let deadline = Instant::now() + PATIENCE;
        while pid_lock_holder(lock) != holder {
            assert!(
                Instant::now() < deadline,
                "the stand-in never took the lock"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The stand-in's pid, which is what it writes into the lock.
    fn pid(&self) -> Pid {
        Pid::new(self.0.id()).expect("a child's pid")
    }

    /// Whether the stand-in has exited, reaping it if so.
    fn exited(&mut self) -> bool {
        self.0.try_wait().expect("its status").is_some()
    }
}

impl Drop for StandIn {
    /// Kill and reap the stand-in, so that no test leaves a process behind, even one that failed.
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn with_nobody_holding_the_lock_nothing_is_ended() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let lock = directory.path().join("tray.pid");

    assert_eq!(
        end_running(&lock, Duration::from_millis(300)),
        Ok(TrayEnded::NoneRunning)
    );
}

#[test]
fn the_tray_holding_the_lock_is_ended_and_its_lock_released() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let lock = directory.path().join("tray.pid");
    let mut tray = StandIn::holding(&lock, "");

    let ended = end_running(&lock, PATIENCE);

    assert_eq!(ended, Ok(TrayEnded::Ended { pid: tray.pid() }));
    assert_eq!(pid_lock_holder(&lock), PidLockHolder::Free);
    let deadline = Instant::now() + PATIENCE;
    while !tray.exited() {
        assert!(Instant::now() < deadline, "the stand-in never exited");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_tray_that_will_not_end_in_time_is_named_and_left_holding_its_lock() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let lock = directory.path().join("tray.pid");
    let mut tray = StandIn::holding(&lock, "trap '' TERM");

    let ended = end_running(&lock, Duration::from_millis(300));

    assert_eq!(
        ended,
        Err(TrayNotEnded {
            pid: Some(tray.pid())
        })
    );
    assert!(!tray.exited(), "it ignored the request");
    assert_eq!(
        pid_lock_holder(&lock),
        PidLockHolder::Held {
            pid: Some(tray.pid())
        }
    );
}

#[test]
fn a_lock_whose_holder_wrote_no_pid_is_not_ended_on_a_guess() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let lock = directory.path().join("tray.pid");
    let held = keepster_service::lock_pid_file(&lock).expect("the lock");
    std::fs::write(&lock, b"").expect("no pid written yet");

    let ended = end_running(&lock, Duration::from_millis(300));

    assert_eq!(ended, Err(TrayNotEnded { pid: None }));
    drop(held);
}

#[test]
fn a_holder_slow_to_write_its_pid_is_waited_for_and_then_ended() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let lock = directory.path().join("tray.pid");
    let mut tray = StandIn::holding_unnamed(&lock, "sleep 0.5");

    let ended = end_running(&lock, PATIENCE);

    assert_eq!(ended, Ok(TrayEnded::Ended { pid: tray.pid() }));
    let deadline = Instant::now() + PATIENCE;
    while !tray.exited() {
        assert!(Instant::now() < deadline, "the stand-in never exited");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_holder_already_gone_when_asked_is_waited_for_until_its_lock_is_released() {
    // The pid written is that of a process already exited and reaped, so asking it to end finds nobody;
    // the lock, held here, is released a moment later, as a dying holder's is.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let lock = directory.path().join("tray.pid");
    let mut gone = Command::new("true").spawn().expect("a child");
    gone.wait().expect("reaped");
    let gone = Pid::new(gone.id()).expect("a child's pid");
    let held = keepster_service::lock_pid_file(&lock).expect("the lock");
    std::fs::write(&lock, gone.to_string()).expect("the gone pid written");
    let releasing = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        drop(held);
    });

    let ended = end_running(&lock, PATIENCE);

    releasing.join().expect("released");
    assert_eq!(ended, Ok(TrayEnded::Ended { pid: gone }));
}
