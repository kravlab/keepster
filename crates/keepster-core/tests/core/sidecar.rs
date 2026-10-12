//! The metadata sidecar's grammar (ticket 48, ADR 0006): one line-oriented blob at a Snapshot's tree root
//! holding every per-path fact git cannot express, byte-identical whenever nothing changed.

use keepster_core::{
    Header, Incomplete, ModeKind, Record, RootPath, Sidecar, SidecarError, SpecialKind,
    WithheldReason,
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
                 x 0600 .ssh/id_rsa\n\
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
            "x 0600 .ssh/id_rsa",
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

fn mode(path: &str, mode: u32) -> Record {
    Record::Mode {
        path: rp(path),
        mode,
    }
}

#[test]
fn a_mode_is_a_record_of_its_own_written_in_four_octal_digits() {
    let sidecar = Sidecar::with_records(vec![
        withheld(
            "tmp/socket",
            WithheldReason::SpecialFile(SpecialKind::Socket),
        ),
        mode(".ssh/id_rsa", 0o600),
        mode("bin", 0o4755),
        incomplete(".ssh/id_rsa", Incomplete::Unstable),
    ]);

    let bytes = sidecar.to_bytes();
    assert_eq!(
        String::from_utf8(bytes.clone()).expect("text"),
        "keepster-meta 1 file=0644 dir=0755\n\
         i unstable .ssh/id_rsa\n\
         m 0600 .ssh/id_rsa\n\
         m 4755 bin\n\
         w special_file:socket tmp/socket\n"
    );
    assert_eq!(Sidecar::parse(&bytes).expect("parses").sidecar, sidecar);
}

#[test]
fn a_path_without_a_mode_record_has_its_kind_s_default() {
    let header = Header {
        file_mode: 0o664,
        dir_mode: 0o775,
        observed: true,
        ..Header::default()
    };
    let sidecar = Sidecar {
        header,
        records: vec![mode("key", 0o600)],
    };
    let modes = sidecar.modes();

    assert_eq!(modes.mode_of(&rp("key"), ModeKind::File), Some(0o600));
    assert_eq!(modes.mode_of(&rp("notes.txt"), ModeKind::File), Some(0o664));
    assert_eq!(
        modes.mode_of(&rp("run.sh"), ModeKind::Executable),
        Some(0o775),
        "an executable file's default is the file default with `x` wherever it has `r`"
    );
    assert_eq!(modes.mode_of(&rp("src"), ModeKind::Directory), Some(0o775));
    assert_eq!(Header::default().default_mode(ModeKind::Executable), 0o755);
    assert_eq!(
        Header {
            file_mode: 0o600,
            ..Header::default()
        }
        .default_mode(ModeKind::Executable),
        0o700
    );
}

#[test]
fn a_snapshot_that_observed_no_modes_has_none_to_give() {
    let unobserved = Sidecar::parse(b"keepster-meta 1 file=0644 dir=0755\n")
        .expect("parses")
        .sidecar;
    assert_eq!(
        unobserved.modes().mode_of(&rp("key"), ModeKind::File),
        None,
        "taken on Windows or before modes were captured: the defaults describe nothing"
    );

    let observed = Sidecar {
        header: Header::modal([]),
        records: Vec::new(),
    };
    let bytes = observed.to_bytes();
    assert_eq!(bytes, b"keepster-meta 1 file=0644 dir=0755 modes\n");
    assert_eq!(Sidecar::parse(&bytes).expect("parses").sidecar, observed);
}

#[test]
fn the_header_s_defaults_are_the_modes_most_paths_have() {
    let header = Header::modal([
        (ModeKind::File, 0o644),
        (ModeKind::File, 0o664),
        (ModeKind::Executable, 0o775),
        (ModeKind::File, 0o600),
        (ModeKind::Directory, 0o755),
        (ModeKind::Directory, 0o700),
        (ModeKind::Directory, 0o700),
    ]);
    assert_eq!(
        (header.file_mode, header.dir_mode),
        (0o664, 0o700),
        "an executable file counts as the file mode it has without `x`"
    );

    let nothing = Header::modal([]);
    assert_eq!(
        nothing,
        Header {
            observed: true,
            ..Header::default()
        },
        "nothing to count keeps the defaults"
    );

    let tied = Header::modal([(ModeKind::File, 0o644), (ModeKind::File, 0o600)]);
    assert_eq!(
        tied.file_mode, 0o600,
        "a tie goes to the lower mode, so it never flips"
    );
}

#[test]
fn every_record_type_is_spelled_as_it_always_was() {
    // ADR 0010: the alphabet is append-only and no existing type changes its meaning — an older build
    // skipping a new type behaves as it always did, while one meeting a redefined type would misread
    // it confidently. Every spelling written today is pinned here; a new fact is a new type.
    let sidecar = Sidecar::with_records(vec![
        withheld("a", WithheldReason::TooLarge),
        withheld("b", WithheldReason::SpecialFile(SpecialKind::Fifo)),
        withheld("c", WithheldReason::SpecialFile(SpecialKind::Socket)),
        withheld("d", WithheldReason::SpecialFile(SpecialKind::CharDevice)),
        withheld("e", WithheldReason::SpecialFile(SpecialKind::BlockDevice)),
        withheld("f", WithheldReason::ReservedName),
        withheld("g", WithheldReason::UnrepresentablePath),
        withheld("h", WithheldReason::OtherRoot),
        withheld("i", WithheldReason::HistoryStore),
        withheld("j", WithheldReason::CloudPlaceholder),
        incomplete("k", Incomplete::Unstable),
        incomplete("l", Incomplete::Error("eacces".to_owned())),
        Record::Mode {
            path: rp("m"),
            mode: 0o600,
        },
    ]);

    assert_eq!(
        String::from_utf8(sidecar.to_bytes()).expect("text"),
        "keepster-meta 1 file=0644 dir=0755\n\
         w too_large a\n\
         w special_file:fifo b\n\
         w special_file:socket c\n\
         w special_file:char_device d\n\
         w special_file:block_device e\n\
         w reserved_name f\n\
         w unrepresentable_path g\n\
         w other_root h\n\
         w history_store i\n\
         w cloud_placeholder j\n\
         i unstable k\n\
         i eacces l\n\
         m 0600 m\n"
    );
    assert_eq!(
        Sidecar::parse(&sidecar.to_bytes()).expect("parses").sidecar,
        sidecar,
        "and every one reads back as itself"
    );
}
