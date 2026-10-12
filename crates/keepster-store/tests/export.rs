//! Export at the store (ticket 58, ticket 19): a Snapshot's versions written into a directory the request
//! names — the Working Tree untouched, a Withheld path absent and said why, nothing overwritten unless
//! confirmed — and the selection a download streams.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use keepster_core::{
    CapturePolicy, Confirmation, Confirmed, Issue, Level, Plan, RootId, RootPath, Rule, RuleOrigin,
    RuleSet, SnapshotId, Step, Trigger, WithheldReason, export_plan,
};
use keepster_store::{ExportScope, HistoryStore, Preflight, Within, Written, destination_naming};

/// 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
}

/// A policy capping files at 64 bytes and excluding `*.log`.
fn policy() -> CapturePolicy {
    let rule = Rule::parse(
        "*.log",
        RootPath::root(),
        RuleOrigin {
            level: Level::Global,
            source: "config.toml".to_owned(),
            line: Some(1),
        },
    )
    .expect("a pattern");
    CapturePolicy::new(RuleSet::new(vec![rule]), RuleSet::default(), 64, Vec::new())
}

struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    store: HistoryStore,
    snapshot: SnapshotId,
}

impl Fixture {
    /// A Snapshot of `notes.txt`, `docs/guide.md` and an over-cap `big.bin`, which it withholds.
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(root.join("docs")).expect("the Root");
        fs::write(root.join("notes.txt"), "kept\n").expect("a file");
        fs::write(root.join("docs").join("guide.md"), "# Guide\n").expect("a file");
        fs::write(root.join("big.bin"), "z".repeat(100)).expect("a file");
        let store = HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store");
        let snapshot = store
            .capture(
                RootId::generate(SystemTime::UNIX_EPOCH + Duration::from_secs(MOMENT)),
                &root,
                &policy(),
                Trigger::Manual,
                SystemTime::UNIX_EPOCH + Duration::from_secs(MOMENT),
            )
            .expect("a Snapshot")
            .id;

        Self {
            directory,
            root,
            store,
            snapshot,
        }
    }

    /// A destination beside the Root, not yet made.
    fn dest(&self) -> PathBuf {
        self.directory.path().join("old")
    }
}

/// The preflight and plan of exporting `paths` into `dest`, with `confirmed`.
fn planned(
    fixture: &Fixture,
    dest: &Path,
    paths: &[RootPath],
    confirmed: &Confirmed,
) -> (Preflight, Plan) {
    let renamed = BTreeMap::new();
    let scope = ExportScope {
        from: fixture.snapshot,
        paths,
        dest,
        naming: destination_naming(fixture.directory.path()),
        renamed: &renamed,
        within: None,
    };
    let preflight = fixture.store.export_preflight(&scope).expect("a preflight");
    let plan = export_plan(
        preflight.found.clone(),
        scope.naming,
        confirmed,
        &BTreeMap::new(),
        false,
        false,
    );

    (preflight, plan)
}

/// Export `paths` into `dest` with `confirmed`, and what happened to each.
fn exported(
    fixture: &Fixture,
    dest: &Path,
    paths: &[RootPath],
    confirmed: &Confirmed,
) -> Vec<(String, Written)> {
    let (preflight, plan) = planned(fixture, dest, paths, confirmed);
    assert!(plan.proceeds(), "{plan:?}");
    let renamed = BTreeMap::new();
    let scope = ExportScope {
        from: fixture.snapshot,
        paths,
        dest,
        naming: destination_naming(fixture.directory.path()),
        renamed: &renamed,
        within: None,
    };
    let mut done = 0;
    let written = fixture
        .store
        .export_write(&scope, &preflight, &plan, &mut |count| done = count);
    assert_eq!(done, plan.paths.len() as u64, "each path is counted");

    plan.paths
        .iter()
        .map(|path| path.path.as_str().to_owned())
        .zip(written)
        .collect()
}

#[test]
fn a_whole_snapshot_lands_in_a_new_directory_and_the_working_tree_is_untouched() {
    let fixture = Fixture::new();
    let dest = fixture.dest();

    let written = exported(&fixture, &dest, &[], &Confirmed::none());

    assert_eq!(
        written,
        [
            ("big.bin".to_owned(), Written::Skipped),
            ("docs/guide.md".to_owned(), Written::Created),
            ("notes.txt".to_owned(), Written::Created),
        ]
    );
    assert_eq!(
        fs::read_to_string(dest.join("notes.txt")).expect("exported"),
        "kept\n"
    );
    assert_eq!(
        fs::read_to_string(dest.join("docs").join("guide.md")).expect("exported"),
        "# Guide\n"
    );
    assert!(
        !dest.join("big.bin").exists(),
        "a Withheld path is absent, never an empty file"
    );
    assert!(
        !dest.join(keepster_core::SIDECAR).exists(),
        "the Snapshot's own record is no file of the Root"
    );
    let names: Vec<String> = fs::read_dir(&dest)
        .expect("the destination")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        names
            .iter()
            .all(|name| !name.starts_with(".keepster-export")),
        "no temporary file is left: {names:?}"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("notes.txt")).expect("kept"),
        "kept\n"
    );
}

#[test]
fn a_destination_not_made_yet_is_made_for_a_single_file() {
    let fixture = Fixture::new();
    let dest = fixture.dest().join("deeper");

    let written = exported(&fixture, &dest, &[rp("notes.txt")], &Confirmed::none());

    assert_eq!(written, [("notes.txt".to_owned(), Written::Created)]);
    assert_eq!(
        fs::read_to_string(dest.join("notes.txt")).expect("exported"),
        "kept\n"
    );
}

#[test]
fn a_withheld_path_asked_for_by_name_is_skipped_with_its_reason() {
    let fixture = Fixture::new();

    let (_, plan) = planned(
        &fixture,
        &fixture.dest(),
        &[rp("big.bin")],
        &Confirmed::none(),
    );

    assert_eq!(plan.paths.len(), 1);
    assert_eq!(plan.paths[0].step, Step::Skipped);
    assert_eq!(
        plan.paths[0].issues,
        [Issue::Withheld {
            reason: WithheldReason::TooLarge
        }]
    );
}

#[test]
fn overwriting_is_confirmed_and_a_file_already_holding_the_version_is_left() {
    let fixture = Fixture::new();
    let dest = fixture.dest();
    fs::create_dir_all(&dest).expect("the destination");
    fs::write(dest.join("notes.txt"), "mine\n").expect("a file of the user's");
    fs::create_dir_all(dest.join("docs")).expect("a directory");
    fs::write(dest.join("docs").join("guide.md"), "# Guide\n").expect("the same version");

    let (_, blocked) = planned(&fixture, &dest, &[rp("notes.txt")], &Confirmed::none());
    assert_eq!(blocked.paths[0].step, Step::Blocked);
    assert_eq!(blocked.paths[0].issues, [Issue::Overwrite]);
    assert_eq!(
        fs::read_to_string(dest.join("notes.txt")).expect("untouched"),
        "mine\n"
    );

    let written = exported(
        &fixture,
        &dest,
        &[rp("notes.txt"), rp("docs")],
        &Confirmed::these([Confirmation::Overwrite]),
    );
    assert_eq!(
        written,
        [
            ("docs/guide.md".to_owned(), Written::Unchanged),
            ("notes.txt".to_owned(), Written::Restored),
        ]
    );
    assert_eq!(
        fs::read_to_string(dest.join("notes.txt")).expect("overwritten"),
        "kept\n"
    );
}

#[test]
fn inside_a_root_overwriting_what_its_snapshot_cannot_hold_is_asked_about() {
    let fixture = Fixture::new();
    // The destination is `old/` inside the Root, whose rules leave `old/` out of every Snapshot.
    let dest = fixture.root.join("old");
    fs::create_dir_all(&dest).expect("the destination");
    fs::write(dest.join("notes.txt"), "an earlier export").expect("in the way");
    let excluding = CapturePolicy::new(
        RuleSet::new(vec![
            Rule::parse(
                "old/",
                RootPath::root(),
                RuleOrigin {
                    level: Level::Root,
                    source: ".keepster.toml".to_owned(),
                    line: Some(2),
                },
            )
            .expect("a pattern"),
        ]),
        RuleSet::default(),
        64,
        Vec::new(),
    );
    let at = rp("old");
    let paths = [rp("notes.txt")];
    let renamed = BTreeMap::new();
    let scope = ExportScope {
        from: fixture.snapshot,
        paths: &paths,
        dest: &dest,
        naming: destination_naming(&dest),
        renamed: &renamed,
        within: Some(Within {
            root_path: &fixture.root,
            policy: &excluding,
            at: &at,
        }),
    };

    let preflight = fixture.store.export_preflight(&scope).expect("a preflight");
    let plan = export_plan(
        preflight.found,
        scope.naming,
        &Confirmed::these([Confirmation::Overwrite, Confirmation::DestInRoot]),
        &BTreeMap::new(),
        false,
        true,
    );

    assert_eq!(
        plan.paths[0].issues,
        [Issue::OverwriteUnrecoverable {
            why: keepster_core::Unrecoverable::Excluded
        }]
    );
}

#[test]
fn a_selection_names_its_files_with_their_sizes_and_what_it_withholds() {
    let fixture = Fixture::new();

    let selection = fixture
        .store
        .selection(fixture.snapshot, &[])
        .expect("a selection");

    let files: Vec<(String, u64)> = selection
        .files
        .iter()
        .map(|file| (file.path.as_str().to_owned(), file.size))
        .collect();
    assert_eq!(
        files,
        [("docs/guide.md".to_owned(), 8), ("notes.txt".to_owned(), 5)]
    );
    assert_eq!(
        selection.withheld,
        [(rp("big.bin"), WithheldReason::TooLarge)]
    );
    let mut bytes = Vec::new();
    fixture
        .store
        .read_selected(&selection.files[1], &mut |chunk| {
            bytes.extend_from_slice(chunk);
            Ok(())
        })
        .expect("its bytes");
    assert_eq!(bytes, b"kept\n");

    let named = fixture
        .store
        .selection(fixture.snapshot, &[rp("big.bin")])
        .expect("a Withheld path asked for by name is no error");
    assert!(named.files.is_empty());
    assert_eq!(named.withheld.len(), 1);
}

#[cfg(unix)]
#[test]
fn exported_files_carry_the_snapshot_s_permission_bits() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let mode = |path: &Path| fs::metadata(path).expect("a file").permissions().mode() & 0o7777;
    fs::set_permissions(
        fixture.root.join("notes.txt"),
        fs::Permissions::from_mode(0o600),
    )
    .expect("private");
    fs::set_permissions(
        fixture.root.join("docs").join("guide.md"),
        fs::Permissions::from_mode(0o755),
    )
    .expect("executable");
    let snapshot = fixture
        .store
        .capture(
            RootId::generate(SystemTime::UNIX_EPOCH + Duration::from_secs(MOMENT)),
            &fixture.root,
            &policy(),
            Trigger::Manual,
            SystemTime::UNIX_EPOCH + Duration::from_secs(MOMENT + 60),
        )
        .expect("a Snapshot")
        .id;
    let fixture = Fixture {
        snapshot,
        ..fixture
    };
    let dest = fixture.dest();

    exported(&fixture, &dest, &[], &Confirmed::none());

    assert_eq!(
        mode(&dest.join("notes.txt")),
        0o600,
        "a private file is not downgraded on the way out"
    );
    assert_eq!(mode(&dest.join("docs").join("guide.md")), 0o755);
}
