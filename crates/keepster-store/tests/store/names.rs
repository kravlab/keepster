//! Names are bytes (ticket 74): a file whose name is not UTF-8 is captured, listed, read back, diffed and
//! restored like any other, and never leaves its Root's Snapshots incomplete. Such names exist on Linux;
//! on Windows the one name that is not text, an unpaired surrogate, is Withheld instead.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, RootId, Trigger};
use keepster_store::{CapturedSnapshot, HistoryStore};

/// 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

/// A Root and its history, side by side in one temporary directory.
struct Fixture {
    _directory: tempfile::TempDir,
    root: PathBuf,
    store: HistoryStore,
    id: RootId,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(&root).expect("the Root");
        let store = HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store");

        Self {
            _directory: directory,
            root,
            store,
            id: RootId::generate(at(MOMENT)),
        }
    }

    /// Write `content` at the path whose components are `names`, each exactly as given.
    fn write(&self, names: &[OsString], content: &[u8]) -> PathBuf {
        let path = names
            .iter()
            .fold(self.root.clone(), |path, name| path.join(name));
        fs::create_dir_all(path.parent().expect("a parent")).expect("its directory");
        fs::write(&path, content).expect("a file");
        path
    }

    fn capture(&self, seconds: u64) -> CapturedSnapshot {
        let captured = self
            .store
            .capture(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(seconds),
            )
            .expect("a Snapshot");
        assert_eq!(captured.index_lag, None, "the index kept up");
        captured
    }
}

#[cfg(target_os = "linux")]
mod unix {
    use std::ffi::OsString;
    use std::fs;
    use std::os::unix::ffi::OsStringExt;

    use keepster_core::{RootPath, SnapshotId};
    use keepster_store::{Depth, EntryKind, HistoryStore};

    use super::{Fixture, MOMENT};

    pub(super) fn name(bytes: &[u8]) -> OsString {
        OsString::from_vec(bytes.to_vec())
    }

    pub(super) fn odd(bytes: &[u8]) -> RootPath {
        RootPath::parse_bytes(bytes).expect("a Root-relative path")
    }

    pub(super) fn read(store: &HistoryStore, snapshot: SnapshotId, path: &RootPath) -> Vec<u8> {
        let content = store.content(snapshot, path).expect("the version");
        let mut whole = Vec::new();
        content
            .read(0, content.size(), &mut |chunk| {
                whole.extend_from_slice(chunk);
                Ok(())
            })
            .expect("a whole read");
        whole
    }

    #[test]
    fn a_name_that_is_not_utf8_is_captured_and_the_snapshot_is_whole() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"\xff.bin")], b"odd");
        fixture.write(&[name(b"\xfe"), name(b"inner.txt")], b"inside");
        fixture.write(&[name(b"plain.txt")], b"plain");

        let captured = fixture.capture(MOMENT);

        assert!(captured.incomplete.is_empty(), "{:?}", captured.incomplete);
        assert!(captured.withheld.is_empty(), "{:?}", captured.withheld);
        assert_eq!(read(&fixture.store, captured.id, &odd(b"\xff.bin")), b"odd");
        assert_eq!(
            read(&fixture.store, captured.id, &odd(b"\xfe/inner.txt")),
            b"inside"
        );
    }

    #[test]
    fn a_snapshot_lists_such_names_by_their_bytes_at_any_depth() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"\xff.bin")], b"odd");
        fixture.write(&[name(b"\xfe"), name(b"\xfd.txt")], b"inside");
        let captured = fixture.capture(MOMENT);

        let top = fixture
            .store
            .entries(captured.id, &RootPath::root(), Depth::One)
            .expect("the top level");
        let named: Vec<(&[u8], EntryKind)> = top
            .entries
            .iter()
            .map(|entry| (entry.path.as_bytes(), entry.kind))
            .collect();
        assert_eq!(
            named,
            [
                (&b"\xfe"[..], EntryKind::Directory),
                (&b"\xff.bin"[..], EntryKind::File)
            ]
        );

        let everything = fixture
            .store
            .entries(captured.id, &RootPath::root(), Depth::Infinity)
            .expect("everything");
        let paths: Vec<&[u8]> = everything
            .entries
            .iter()
            .map(|entry| entry.path.as_bytes())
            .collect();
        assert_eq!(paths, [&b"\xfe"[..], b"\xfe/\xfd.txt", b"\xff.bin"]);

        let inside = fixture
            .store
            .entries(captured.id, &odd(b"\xfe"), Depth::One)
            .expect("the odd directory");
        assert_eq!(inside.entries.len(), 1);
        assert_eq!(inside.entries[0].path.as_bytes(), b"\xfe/\xfd.txt");
    }

    #[test]
    fn two_names_that_read_alike_as_text_are_two_files() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"\xff")], b"first");
        fixture.write(&[name(b"\xfe")], b"second");

        let captured = fixture.capture(MOMENT);

        assert!(captured.incomplete.is_empty(), "{:?}", captured.incomplete);
        assert_eq!(read(&fixture.store, captured.id, &odd(b"\xff")), b"first");
        assert_eq!(read(&fixture.store, captured.id, &odd(b"\xfe")), b"second");
    }

    #[test]
    fn such_a_file_leaves_the_next_snapshot_when_it_is_deleted() {
        let fixture = Fixture::new();
        let written = fixture.write(&[name(b"\xff.bin")], b"odd");
        fixture.write(&[name(b"plain.txt")], b"plain");
        fixture.capture(MOMENT);

        fs::remove_file(written).expect("deleted");
        let after = fixture.capture(MOMENT + 60);

        let listed = fixture
            .store
            .entries(after.id, &RootPath::root(), Depth::Infinity)
            .expect("the listing");
        let paths: Vec<&[u8]> = listed
            .entries
            .iter()
            .map(|entry| entry.path.as_bytes())
            .collect();
        assert_eq!(paths, [&b"plain.txt"[..]]);
    }

    #[test]
    fn such_a_file_keeps_its_permission_bits() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new();
        let written = fixture.write(&[name(b"\xff.sh")], b"#!/bin/sh\n");
        fs::set_permissions(&written, fs::Permissions::from_mode(0o750)).expect("chmod");

        let captured = fixture.capture(MOMENT);

        let listed = fixture
            .store
            .entries(captured.id, &RootPath::root(), Depth::One)
            .expect("the listing");
        assert_eq!(listed.entries[0].permissions, Some(0o750));
    }

    #[test]
    fn a_name_alike_as_text_to_an_existing_file_is_not_found_in_its_place() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"\xff.bin")], b"odd");
        let captured = fixture.capture(MOMENT);

        assert!(
            fixture
                .store
                .content(
                    captured.id,
                    &RootPath::parse("\u{fffd}.bin").expect("valid")
                )
                .is_err(),
            "only the exact bytes name the file"
        );
    }
}

#[cfg(target_os = "linux")]
mod operations {
    use std::collections::BTreeMap;
    use std::fs;
    use std::ops::ControlFlow;
    use std::os::unix::ffi::OsStrExt;

    use keepster_core::{
        CapturePolicy, Confirmed, ContentQuery, Level, PathQuery, PreviewClass, RootPath, Rule,
        RuleOrigin, plan,
    };
    use keepster_store::{
        ContentFound, DiffTarget, FileChange, PreviewOptions, RestoreScope, TimelineQuery, Written,
        destination_naming, preview,
    };

    use super::unix::{name, odd, read};
    use super::{Fixture, MOMENT};

    #[test]
    fn the_timeline_of_such_a_path_finds_the_snapshots_that_changed_it() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"\xff.bin")], b"one");
        fixture.write(&[name(b"\xfe.bin")], b"other");
        let first = fixture.capture(MOMENT);
        fixture.write(&[name(b"\xfe.bin")], b"changed");
        fixture.capture(MOMENT + 60);
        fixture.write(&[name(b"\xff.bin")], b"two");
        let third = fixture.capture(MOMENT + 120);

        let page = fixture
            .store
            .timeline(&TimelineQuery {
                path: Some(odd(b"\xff.bin")),
                before: None,
                not_before: None,
                limit: 10,
            })
            .expect("the timeline");

        let ids: Vec<_> = page.snapshots.iter().map(|snapshot| snapshot.id).collect();
        assert_eq!(ids, [third.id, first.id], "only the name's own bytes match");
    }

    #[test]
    fn a_search_finds_such_a_path_by_name_and_by_content() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"docs"), name(b"\xff report.txt")], b"needle here\n");
        fixture.write(&[name(b"docs"), name(b"plain.txt")], b"nothing\n");
        fixture.capture(MOMENT);

        let (found, more) = fixture
            .store
            .find_paths(&PathQuery::new("report", false).expect("a query"), 10)
            .expect("found");
        assert!(!more);
        let paths: Vec<&[u8]> = found.iter().map(|hit| hit.path.as_bytes()).collect();
        assert_eq!(paths, [&b"docs/\xff report.txt"[..]]);

        let plan = fixture.store.content_plan(&odd(b"docs")).expect("a plan");
        let mut matched = Vec::new();
        fixture.store.search_content(
            &plan,
            &ContentQuery::new("needle", false).expect("a query"),
            &mut |hit| {
                if let ContentFound::Match { version, .. } = hit {
                    matched.push(version.path.as_bytes().to_vec());
                }
                ControlFlow::Continue(())
            },
        );
        assert_eq!(matched, [b"docs/\xff report.txt".to_vec()]);
    }

    #[test]
    fn a_diff_names_such_a_file_by_its_bytes_and_its_patch_quotes_it_as_git_does() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new();
        let written = fixture.write(&[name(b"\xff.txt")], b"one\n");
        fixture.write(&[name(b"\xfe.sh")], b"#!/bin/sh\n");
        let before = fixture.capture(MOMENT);
        fs::write(&written, b"two\n").expect("edited");
        fs::set_permissions(
            fixture.root.join(std::ffi::OsStr::from_bytes(b"\xfe.sh")),
            fs::Permissions::from_mode(0o700),
        )
        .expect("chmod");
        let after = fixture.capture(MOMENT + 60);

        let comparison = fixture
            .store
            .diff(before.id, &DiffTarget::Snapshot(after.id), None)
            .expect("a diff");
        let files: Vec<(&[u8], Option<FileChange>)> = comparison
            .files
            .iter()
            .map(|file| (file.path.as_bytes(), file.change))
            .collect();
        assert_eq!(
            files,
            [
                (&b"\xfe.sh"[..], Some(FileChange::Modified)),
                (&b"\xff.txt"[..], Some(FileChange::Modified))
            ]
        );

        let one = fixture
            .store
            .diff(
                before.id,
                &DiffTarget::Snapshot(after.id),
                Some(&odd(b"\xff.txt")),
            )
            .expect("one file");
        assert_eq!(one.files.len(), 1);
        assert!(!one.files[0].hunks.is_empty(), "{:?}", one.files[0]);

        let patch = fixture
            .store
            .unified(before.id, &DiffTarget::Snapshot(after.id), None)
            .expect("a patch");
        let text = String::from_utf8(patch).expect("every name is quoted, so the patch is text");
        assert!(
            text.starts_with(
                "diff --git \"a/\\376.sh\" \"b/\\376.sh\"\nold mode 100644\nnew mode 100700\n"
            ),
            "the header this code writes: {text}"
        );
        assert!(
            text.contains("diff --git \"a/\\377.txt\" \"b/\\377.txt\"\n"),
            "the header libgit2 writes: {text}"
        );
    }

    #[test]
    fn a_restore_writes_such_a_file_back_under_its_exact_name() {
        let fixture = Fixture::new();
        let written = fixture.write(&[name(b"\xfe"), name(b"\xff.bin")], b"saved");
        let saved = fixture.capture(MOMENT);
        fs::remove_file(&written).expect("deleted");

        let paths = [odd(b"\xfe/\xff.bin")];
        let renamed = BTreeMap::new();
        let policy = CapturePolicy::default();
        let scope = RestoreScope {
            from: saved.id,
            paths: &paths,
            root_path: &fixture.root,
            policy: &policy,
            naming: destination_naming(&fixture.root),
            renamed: &renamed,
            link_names: None,
        };
        let preflight = fixture
            .store
            .restore_preflight(&scope)
            .expect("a preflight");
        let planned = plan(
            preflight.found.clone(),
            scope.naming,
            &Confirmed::none(),
            &BTreeMap::new(),
            false,
        );
        assert!(planned.proceeds(), "{planned:?}");
        let outcome = fixture.store.restore_write(&scope, &preflight, &planned);

        assert_eq!(outcome, [Written::Created]);
        assert_eq!(fs::read(&written).expect("the file is back"), b"saved");
    }

    #[test]
    fn an_export_selection_names_such_a_file_by_its_bytes() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"\xfe"), name(b"\xff.bin")], b"saved");
        let saved = fixture.capture(MOMENT);

        let selection = fixture
            .store
            .selection(saved.id, &[odd(b"\xfe")])
            .expect("a selection");

        let paths: Vec<&[u8]> = selection
            .files
            .iter()
            .map(|file| file.path.as_bytes())
            .collect();
        assert_eq!(paths, [&b"\xfe/\xff.bin"[..]]);
    }

    #[test]
    fn forget_removes_such_a_file_by_a_pattern_around_its_bytes() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"\xff.secret")], b"hunter2");
        fixture.write(&[name(b"notes.txt")], b"kept");
        let saved = fixture.capture(MOMENT);
        let pattern = Rule::parse(
            "*.secret",
            RootPath::root(),
            RuleOrigin {
                level: Level::Forget,
                source: "keepster forget".to_owned(),
                line: None,
            },
        )
        .expect("a pattern");

        let forgotten = fixture
            .store
            .forget(&pattern, Some(&fixture.root), &mut |_, _| {})
            .expect("forgotten");

        assert_eq!(forgotten.paths, [odd(b"\xff.secret")]);
        assert_eq!(forgotten.still_on_disk, [odd(b"\xff.secret")]);
        assert!(
            fixture
                .store
                .content(saved.id, &odd(b"\xff.secret"))
                .is_err()
        );
        assert_eq!(read(&fixture.store, saved.id, &odd(b"notes.txt")), b"kept");
    }

    #[test]
    fn a_capture_preview_names_such_a_file_by_its_bytes_and_captures_it() {
        let fixture = Fixture::new();
        fixture.write(&[name(b"\xff.bin")], b"odd");

        let mut seen = Vec::new();
        preview(
            &fixture.root,
            &RootPath::root(),
            &CapturePolicy::default(),
            &PreviewOptions {
                measure_excluded: false,
            },
            &mut |entry| {
                seen.push((entry.path.as_bytes().to_vec(), entry.class));
                ControlFlow::Continue(())
            },
        )
        .expect("a walk");

        assert_eq!(seen, [(b"\xff.bin".to_vec(), PreviewClass::Captured)]);
    }
}

#[cfg(windows)]
#[test]
fn on_windows_a_name_with_an_unpaired_surrogate_is_withheld_and_the_snapshot_whole() {
    use std::os::windows::ffi::OsStringExt;

    use keepster_core::WithheldReason;

    let fixture = Fixture::new();
    // `\u{D800}x.t`: no UTF-8 can spell the lone surrogate, so git cannot hold the name.
    let surrogate =
        OsString::from_wide(&[0xD800, u16::from(b'x'), u16::from(b'.'), u16::from(b't')]);
    fixture.write(&[surrogate], b"unholdable");
    fixture.write(&[OsString::from("plain.txt")], b"plain");

    let captured = fixture.capture(MOMENT);

    assert!(captured.incomplete.is_empty(), "{:?}", captured.incomplete);
    let withheld: Vec<(String, WithheldReason)> = captured
        .withheld
        .iter()
        .map(|withheld| (withheld.path.to_lossy().into_owned(), withheld.reason))
        .collect();
    assert_eq!(
        withheld,
        [(
            "\u{fffd}x.t".to_owned(),
            WithheldReason::UnrepresentablePath
        )]
    );
}
