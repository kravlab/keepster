//! The log's rotation (ticket 62), over real files on every platform: rotation on size, a bounded number
//! of files, a record never split across two, and what renaming a file someone else has open does —
//! which on Windows is not what it does anywhere else.

use std::path::Path;

use keepster_service::rotating::{RotatingFile, Tail, tail};
use tempfile::TempDir;

const NAME: &str = "daemon.log";

fn read(directory: &Path, name: &str) -> Option<String> {
    std::fs::read_to_string(directory.join(name)).ok()
}

/// Every file the log consists of, oldest first, concatenated: what a person reading them in order sees.
fn everything(directory: &Path, keep: usize) -> String {
    let mut all = String::new();
    for n in (1..=keep).rev() {
        all.push_str(&read(directory, &format!("{NAME}.{n}")).unwrap_or_default());
    }
    all.push_str(&read(directory, NAME).unwrap_or_default());
    all
}

#[test]
fn a_record_that_fills_the_file_exactly_does_not_rotate_it() {
    let directory = TempDir::new().expect("a directory");
    let log = RotatingFile::open(directory.path(), NAME, 10, 2).expect("opened");

    log.write_record(b"1234").expect("written");
    log.write_record(b"567890").expect("written");

    assert_eq!(read(directory.path(), NAME).as_deref(), Some("1234567890"));
    assert_eq!(read(directory.path(), &format!("{NAME}.1")), None);
}

#[test]
fn a_record_past_the_boundary_starts_a_new_file_and_is_never_split() {
    let directory = TempDir::new().expect("a directory");
    let log = RotatingFile::open(directory.path(), NAME, 10, 2).expect("opened");

    log.write_record(b"first\n").expect("written");
    log.write_record(b"second\n").expect("written");

    assert_eq!(read(directory.path(), NAME).as_deref(), Some("second\n"));
    assert_eq!(
        read(directory.path(), &format!("{NAME}.1")).as_deref(),
        Some("first\n")
    );
}

#[test]
fn only_the_newest_files_are_kept_and_the_oldest_goes_first() {
    let directory = TempDir::new().expect("a directory");
    let log = RotatingFile::open(directory.path(), NAME, 10, 2).expect("opened");

    for record in [
        "aaaaaaaaa\n",
        "bbbbbbbbb\n",
        "ccccccccc\n",
        "ddddddddd\n",
        "eeeeeeeee\n",
    ] {
        log.write_record(record.as_bytes()).expect("written");
    }

    assert_eq!(read(directory.path(), NAME).as_deref(), Some("eeeeeeeee\n"));
    assert_eq!(
        read(directory.path(), &format!("{NAME}.1")).as_deref(),
        Some("ddddddddd\n")
    );
    assert_eq!(
        read(directory.path(), &format!("{NAME}.2")).as_deref(),
        Some("ccccccccc\n")
    );
    assert_eq!(
        read(directory.path(), &format!("{NAME}.3")),
        None,
        "two rotated files, no more"
    );
}

#[test]
fn a_record_larger_than_a_file_is_written_whole_in_a_file_of_its_own() {
    let directory = TempDir::new().expect("a directory");
    let log = RotatingFile::open(directory.path(), NAME, 4, 2).expect("opened");

    log.write_record(b"ab").expect("written");
    log.write_record(b"0123456789").expect("written");
    log.write_record(b"cd").expect("written");

    assert_eq!(
        read(directory.path(), &format!("{NAME}.2")).as_deref(),
        Some("ab")
    );
    assert_eq!(
        read(directory.path(), &format!("{NAME}.1")).as_deref(),
        Some("0123456789")
    );
    assert_eq!(read(directory.path(), NAME).as_deref(), Some("cd"));
}

#[test]
fn a_log_opened_again_appends_and_counts_what_is_already_there() {
    let directory = TempDir::new().expect("a directory");
    let log = RotatingFile::open(directory.path(), NAME, 10, 2).expect("opened");
    log.write_record(b"before\n").expect("written");
    drop(log);

    let log = RotatingFile::open(directory.path(), NAME, 10, 2).expect("opened again");
    log.write_record(b"after\n").expect("written");

    assert_eq!(
        read(directory.path(), &format!("{NAME}.1")).as_deref(),
        Some("before\n")
    );
    assert_eq!(read(directory.path(), NAME).as_deref(), Some("after\n"));
}

#[test]
fn a_reader_holding_the_log_open_costs_no_record_when_it_rotates() {
    // What `tail -f`, an editor or the diagnostics bundle does to a log: holds it open while it rotates.
    let directory = TempDir::new().expect("a directory");
    let log = RotatingFile::open(directory.path(), NAME, 10, 3).expect("opened");
    log.write_record(b"one\n").expect("written");
    let reader = std::fs::File::open(directory.path().join(NAME)).expect("a reader");

    for record in ["two\n", "three\n", "four\n"] {
        log.write_record(record.as_bytes())
            .expect("written past the boundary");
    }
    drop(reader);

    assert_eq!(everything(directory.path(), 3), "one\ntwo\nthree\nfour\n");
}

/// On Windows a file another handle holds without sharing deletion cannot be renamed, and rotating is a
/// rename. The log goes on growing past its size rather than lose a record, and rotates once it can.
#[cfg(windows)]
#[test]
fn a_reader_that_forbids_renaming_delays_the_rotation_and_costs_nothing() {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let directory = TempDir::new().expect("a directory");
    let log = RotatingFile::open(directory.path(), NAME, 10, 3).expect("opened");
    log.write_record(b"aaaaaaaaa\n").expect("written");
    log.write_record(b"one\n").expect("written, rotating once");
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(directory.path().join(NAME))
        .expect("a reader that forbids deletion");

    log.write_record(b"two\n").expect("written");
    log.write_record(b"three\n")
        .expect("written although the file could not be rotated");
    log.write_record(b"three and a half\n")
        .expect("written, the rotation tried again");
    assert_eq!(
        read(directory.path(), &format!("{NAME}.1")).as_deref(),
        Some("aaaaaaaaa\n"),
        "a rotation that could not rename the current file left the older ones as they were"
    );
    assert_eq!(read(directory.path(), &format!("{NAME}.2")), None);
    drop(reader);
    log.write_record(b"four\n").expect("written");

    assert_eq!(
        read(directory.path(), &format!("{NAME}.2")).as_deref(),
        Some("aaaaaaaaa\n"),
        "rotated as soon as it could be"
    );
    assert_eq!(
        everything(directory.path(), 3),
        "aaaaaaaaa\none\ntwo\nthree\nthree and a half\nfour\n"
    );
}

/// A reader may hold a rotated file just as well, which on Windows keeps the shift from moving it. The
/// rotation then puts the current file back rather than delete what it had moved aside.
#[cfg(windows)]
#[test]
fn a_reader_holding_a_rotated_file_costs_no_record_either() {
    use std::os::windows::fs::OpenOptionsExt;

    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let directory = TempDir::new().expect("a directory");
    let log = RotatingFile::open(directory.path(), NAME, 10, 3).expect("opened");
    for record in ["aaaaaaaaa\n", "bbbbbbbbb\n", "ccccccccc\n"] {
        log.write_record(record.as_bytes()).expect("written");
    }
    assert!(
        read(directory.path(), &format!("{NAME}.2")).is_some(),
        "two rotations so far"
    );
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(directory.path().join(format!("{NAME}.1")))
        .expect("a reader of the rotated file that forbids deletion");

    // Each record past the boundary tries the rotation again, and each try stops at the held file.
    for record in ["ddddddddd\n", "eeeeeeeee\n", "fffffffff\n"] {
        log.write_record(record.as_bytes())
            .expect("written although the shift could not go through");
    }
    drop(reader);
    log.write_record(b"ggggggggg\n").expect("written");

    assert_eq!(
        everything(directory.path(), 3),
        "aaaaaaaaa\nbbbbbbbbb\nccccccccc\nddddddddd\neeeeeeeee\nfffffffff\nggggggggg\n",
        "three tries at a rotation lost no file"
    );
}

#[test]
fn a_ceiling_is_divided_among_a_fixed_number_of_files() {
    let (size, keep) = keepster_service::rotating::files_for(50 * 1024 * 1024);

    assert_eq!(keep, 4, "the current file and four rotated ones");
    assert_eq!(size * (keep as u64 + 1), 50 * 1024 * 1024);
}

fn write(directory: &Path, name: &str, text: &str) {
    std::fs::write(directory.join(name), text).expect("a log file");
}

#[test]
fn a_tail_within_its_budget_is_every_file_whole_newest_first() {
    let directory = TempDir::new().expect("a directory");
    write(directory.path(), NAME, "c1\nc2\n");
    write(directory.path(), &format!("{NAME}.1"), "b1\n");
    write(directory.path(), &format!("{NAME}.2"), "a1\n");

    let found = tail(directory.path(), NAME, 1024).expect("read");

    assert_eq!(
        found,
        vec![
            Tail {
                name: NAME.to_owned(),
                size: 6,
                text: "c1\nc2\n".to_owned(),
                truncated: false,
            },
            Tail {
                name: format!("{NAME}.1"),
                size: 3,
                text: "b1\n".to_owned(),
                truncated: false,
            },
            Tail {
                name: format!("{NAME}.2"),
                size: 3,
                text: "a1\n".to_owned(),
                truncated: false,
            },
        ]
    );
}

#[test]
fn a_tail_that_spends_its_budget_in_a_file_keeps_its_last_whole_lines_and_stops() {
    let directory = TempDir::new().expect("a directory");
    write(directory.path(), NAME, "new\n");
    write(
        directory.path(),
        &format!("{NAME}.1"),
        "first line\nsecond\nthird\n",
    );
    write(directory.path(), &format!("{NAME}.2"), "never read\n");

    // Four bytes for the current file leave ten: "ond\nthird\n" cuts into "second", which is dropped.
    let found = tail(directory.path(), NAME, 14).expect("read");

    assert_eq!(
        found.len(),
        2,
        "the budget is spent before the oldest file: {found:?}"
    );
    assert_eq!(found[0].text, "new\n");
    assert!(!found[0].truncated);
    assert_eq!(
        found[1],
        Tail {
            name: format!("{NAME}.1"),
            size: 24,
            text: "third\n".to_owned(),
            truncated: true,
        },
        "never half a line"
    );
}

#[test]
fn a_log_never_written_has_no_tail() {
    let directory = TempDir::new().expect("a directory");

    assert_eq!(tail(directory.path(), NAME, 1024).expect("read"), vec![]);
}

#[test]
fn a_rotated_file_that_is_not_there_is_skipped_and_the_older_ones_still_read() {
    let directory = TempDir::new().expect("a directory");
    write(directory.path(), NAME, "newest\n");
    write(directory.path(), &format!("{NAME}.2"), "older\n");

    let found = tail(directory.path(), NAME, 1024).expect("read");

    assert_eq!(
        found
            .iter()
            .map(|tail| (tail.name.as_str(), tail.text.as_str()))
            .collect::<Vec<_>>(),
        [(NAME, "newest\n"), (&*format!("{NAME}.2"), "older\n")]
    );
}
