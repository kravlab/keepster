//! The metadata sidecar's grammar (ticket 48, ADR 0006): one line-oriented blob at a Snapshot's tree root
//! holding every per-path fact git cannot express, byte-identical whenever nothing changed.

use keepster_core::{
    Incomplete, Record, RootPath, Sidecar, SidecarError, SpecialKind, WithheldReason,
};

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
}

fn withheld(path: &str, reason: WithheldReason) -> Record {
    Record::Withheld {
        path: rp(path),
        reason,
    }
}

fn incomplete(path: &str, why: Incomplete) -> Record {
    Record::Incomplete {
        path: rp(path),
        why,
    }
}

#[test]
fn records_are_written_after_the_header_in_path_order_whatever_order_they_came_in() {
    let sidecar = Sidecar::with_records(vec![
        incomplete("src/main.rs", Incomplete::Unstable),
        withheld("var/big.iso", WithheldReason::TooLarge),
        withheld(
            "run/control",
            WithheldReason::SpecialFile(SpecialKind::Fifo),
        ),
        incomplete("private/notes.txt", Incomplete::Error("eacces".to_owned())),
        withheld(".git", WithheldReason::ReservedName),
    ]);

    assert_eq!(
        String::from_utf8(sidecar.to_bytes()).expect("text"),
        "keepster-meta 1 file=0644 dir=0755\n\
         w reserved_name .git\n\
         i eacces private/notes.txt\n\
         w special_file:fifo run/control\n\
         i unstable src/main.rs\n\
         w too_large var/big.iso\n"
    );
}

#[test]
fn what_is_written_reads_back_as_it_was() {
    let sidecar = Sidecar::with_records(vec![
        withheld("a b/c.txt", WithheldReason::HistoryStore),
        withheld(
            "dev/null",
            WithheldReason::SpecialFile(SpecialKind::CharDevice),
        ),
        withheld("other", WithheldReason::OtherRoot),
        incomplete(
            "locked.db",
            Incomplete::Error("sharing_violation".to_owned()),
        ),
    ]);

    let parsed = Sidecar::parse(&sidecar.to_bytes()).expect("its own output parses");

    assert_eq!(parsed.sidecar, sidecar);
    assert!(parsed.unknown.is_empty());
}

#[test]
fn a_path_a_line_cannot_hold_as_it_is_is_quoted_as_git_quotes_it() {
    let awkward = [
        "line\nbreak.txt",
        "tab\there.txt",
        "\"quoted\".txt",
        "back\\slash.txt",
        " leading-space.txt",
        "next\u{85}line.txt",
    ];
    let sidecar = Sidecar::with_records(
        awkward
            .iter()
            .map(|path| withheld(path, WithheldReason::TooLarge))
            .collect(),
    );

    let bytes = sidecar.to_bytes();
    assert_eq!(
        bytes.iter().filter(|byte| **byte == b'\n').count(),
        awkward.len() + 1,
        "one line per record: {}",
        String::from_utf8_lossy(&bytes)
    );
    assert!(String::from_utf8_lossy(&bytes).contains("w too_large \"line\\nbreak.txt\"\n"));
    assert!(
        String::from_utf8_lossy(&bytes).contains("\"next\\302\\205line.txt\""),
        "a control character beyond ASCII is escaped byte by byte, as git escapes it"
    );
    assert_eq!(Sidecar::parse(&bytes).expect("parses").sidecar, sidecar);
}

#[test]
fn a_record_this_build_does_not_know_is_skipped_and_reported() {
    let text = b"keepster-meta 1 file=0644 dir=0755\n\
                 m 0600 .ssh/id_rsa\n\
                 w too_large big.iso\n\
                 w a_reason_from_the_future thing\n\
                 d empty/dir\n";

    let parsed = Sidecar::parse(text).expect("an older build still reads it");

    assert_eq!(
        parsed.sidecar.records,
        [withheld("big.iso", WithheldReason::TooLarge)]
    );
    assert_eq!(
        parsed.unknown,
        [
            "m 0600 .ssh/id_rsa",
            "w a_reason_from_the_future thing",
            "d empty/dir"
        ]
    );
}

#[test]
fn the_header_carries_its_version_and_a_newer_one_is_refused_rather_than_misread() {
    let parsed = Sidecar::parse(b"keepster-meta 1 file=0600 dir=0700\n").expect("parses");
    assert_eq!(
        (
            parsed.sidecar.header.file_mode,
            parsed.sidecar.header.dir_mode
        ),
        (0o600, 0o700)
    );

    assert_eq!(
        Sidecar::parse(b"keepster-meta 2 file=0644 dir=0755\n"),
        Err(SidecarError::Version(2))
    );
    assert_eq!(
        Sidecar::parse(b"not a sidecar\n"),
        Err(SidecarError::Header)
    );
}

#[test]
fn every_reason_and_kind_has_a_stable_wire_name() {
    let reasons = [
        (WithheldReason::TooLarge, "too_large", None),
        (
            WithheldReason::SpecialFile(SpecialKind::Socket),
            "special_file",
            Some("socket"),
        ),
        (
            WithheldReason::SpecialFile(SpecialKind::BlockDevice),
            "special_file",
            Some("block_device"),
        ),
        (WithheldReason::ReservedName, "reserved_name", None),
        (
            WithheldReason::UnrepresentablePath,
            "unrepresentable_path",
            None,
        ),
        (WithheldReason::OtherRoot, "other_root", None),
        (WithheldReason::HistoryStore, "history_store", None),
        (WithheldReason::CloudPlaceholder, "cloud_placeholder", None),
    ];
    for (reason, name, kind) in reasons {
        assert_eq!(reason.as_str(), name);
        assert_eq!(reason.kind().map(SpecialKind::as_str), kind);
    }
}
