//! Who holds a pid file's lock (issue 49): what `keepster install` reads before it ends a running tray,
//! told from the lock itself rather than from what the file says.

use keepster_service::{Pid, PidLockHolder, lock_pid_file, pid_lock_holder};

#[test]
fn a_pid_file_that_does_not_exist_is_free_and_is_not_created() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("tray.pid");

    assert_eq!(pid_lock_holder(&path), PidLockHolder::Free);
    assert!(!path.exists(), "looking must not leave a file behind");
}

#[test]
fn a_pid_file_nobody_locks_is_free_whatever_it_says() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("tray.pid");
    std::fs::write(&path, b"4242").expect("a leftover pid file");

    assert_eq!(pid_lock_holder(&path), PidLockHolder::Free);
}

#[test]
fn a_held_lock_names_the_pid_written_by_its_holder() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("tray.pid");
    let lock = lock_pid_file(&path).expect("the lock");

    assert_eq!(
        pid_lock_holder(&path),
        PidLockHolder::Held {
            pid: Pid::new(std::process::id())
        }
    );
    drop(lock);
    assert_eq!(
        pid_lock_holder(&path),
        PidLockHolder::Free,
        "released with its holder"
    );
}
