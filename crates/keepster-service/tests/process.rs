//! Ending a process by its pid (issue 49): how `keepster install` ends a tray running from an older
//! install, and what it refuses to signal.
#![cfg(unix)]

use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};

use keepster_service::{Pid, end_process};

#[test]
fn a_running_process_is_sent_the_termination_signal() {
    let mut child = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .spawn()
        .expect("a child to end");

    let ended = end_process(Pid::new(child.id()).expect("a child's pid"));
    let status = child.wait().expect("the child is reaped");

    ended.expect("the signal was sent");
    assert_eq!(
        status.signal(),
        Some(15),
        "SIGTERM, not a harder kill: {status}"
    );
}

#[test]
fn pid_zero_cannot_be_named_so_this_process_group_is_never_signalled() {
    assert_eq!(Pid::new(0), None);
}

#[test]
fn a_pid_beyond_the_platforms_range_is_refused_rather_than_read_as_a_process_group() {
    let refused =
        end_process(Pid::new(u32::MAX).expect("not 0")).expect_err("no process has this pid");

    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::InvalidInput,
        "{refused}"
    );
}

#[test]
fn a_process_already_gone_reads_as_not_found() {
    let mut child = Command::new("true").spawn().expect("a child");
    child.wait().expect("reaped");

    let gone =
        end_process(Pid::new(child.id()).expect("a child's pid")).expect_err("nobody to end");

    assert_eq!(gone.kind(), std::io::ErrorKind::NotFound, "{gone}");
}
