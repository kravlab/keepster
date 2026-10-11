//! Permission bits in a Snapshot (ticket 50, ADR 0006): all twelve captured on Unix as deviations from the
//! Root's modal defaults, a permission-only change is a change, and a Restore applies them as asked.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use keepster_core::{
    CapturePolicy, Confirmed, Issue, ModeChoice, Plan, Resolution, RootId, RootPath, SIDECAR,
    SnapshotId, Step, Trigger, Verify, plan,
};
use keepster_store::{
    CaptureRequest, CapturedSnapshot, Depth, DiffTarget, FileChange, HistoryStore, Pass,
    RestoreScope, Written, destination_naming,
};

/// 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
}

struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    store: HistoryStore,
    id: RootId,
}

impl Fixture {
    /// A Root whose every mode is set explicitly, so nothing depends on the umask the tests run under.
    fn new(files: &[(&str, u32)]) -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(&root).expect("the Root");
        for (path, mode) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("its directory");
            fs::write(&path, b"content").expect("a file");
            chmod(&path, *mode);
        }
        for directory in walk_directories(&root) {
            chmod(&directory, 0o755);
        }
        let store = HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store");

        Self {
            directory,
            root,
            store,
            id: RootId::generate(at(MOMENT)),
        }
    }

    fn capture(&self, trigger: Trigger, seconds: u64) -> CapturedSnapshot {
        self.store
            .capture(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                trigger,
                at(seconds),
            )
            .expect("a Snapshot")
    }

    fn pass(&self, seconds: u64) -> Pass {
        self.pass_as(Trigger::Watcher, seconds)
    }

    /// A pass that writes no Snapshot when nothing changed, as every automatic one is.
    fn pass_as(&self, trigger: Trigger, seconds: u64) -> Pass {
        self.store
            .pass(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                &CaptureRequest {
                    trigger,
                    now: at(seconds),
                    verify: Verify::Stat,
                    record_unchanged: false,
                },
            )
            .expect("a pass")
    }

    /// Rewrite the newest Snapshot as a build that observed no modes wrote it — without a sidecar —
    /// and reopen the store, its index rebuilt from git.
    fn as_if_no_modes_were_observed(&mut self) {
        let repository = git2::Repository::open_bare(self.store.path()).expect("the store");
        let mut segment = repository
            .references_glob("refs/keepster/segments/*")
            .expect("the Segments")
            .next()
            .expect("a Segment")
            .expect("readable");
        let commit = segment.peel_to_commit().expect("its tip");
        let mut tree = repository
            .treebuilder(Some(&commit.tree().expect("its tree")))
            .expect("a builder");
        tree.remove(SIDECAR).expect("a sidecar to remove");
        let tree = repository
            .find_tree(tree.write().expect("written"))
            .expect("the tree");
        let parents: Vec<git2::Commit<'_>> = commit.parents().collect();
        let parents: Vec<&git2::Commit<'_>> = parents.iter().collect();
        let rewritten = repository
            .commit(
                None,
                &commit.author(),
                &commit.committer(),
                commit.message().expect("a message"),
                &tree,
                &parents,
            )
            .expect("rewritten");
        segment
            .set_target(rewritten, "as an older build wrote it")
            .expect("moved");

        let index = self.directory.path().join("index.sqlite");
        fs::remove_file(&index).expect("the index");
        self.store = HistoryStore::open(self.directory.path().join("history.git"), index)
            .expect("the store, reopened");
    }

    /// The newest Snapshot's sidecar, as text.
    fn sidecar(&self) -> Option<String> {
        let repository = git2::Repository::open_bare(self.store.path()).expect("the store");
        let mut newest: Option<git2::Commit<'_>> = None;
        for reference in repository
            .references_glob("refs/keepster/segments/*")
            .expect("the Segments")
        {
            let commit = reference
                .expect("a Segment")
                .peel_to_commit()
                .expect("its tip");
            if newest
                .as_ref()
                .is_none_or(|best| commit.time().seconds() >= best.time().seconds())
            {
                newest = Some(commit);
            }
        }
        let tree = newest.expect("a Snapshot").tree().expect("its tree");
        let entry = tree.get_name(SIDECAR)?;
        let blob = repository.find_blob(entry.id()).expect("the sidecar");
        Some(String::from_utf8(blob.content().to_vec()).expect("text"))
    }

    fn mode(&self, path: &str) -> u32 {
        fs::metadata(self.root.join(path))
            .expect("the path")
            .permissions()
            .mode()
            & 0o7777
    }

    fn restore(
        &self,
        from: SnapshotId,
        paths: &[&str],
        confirmed: &Confirmed,
        resolutions: &BTreeMap<RootPath, Resolution>,
    ) -> (Plan, Option<Vec<Written>>) {
        let paths: Vec<RootPath> = paths.iter().map(|path| rp(path)).collect();
        let renamed = BTreeMap::new();
        let policy = CapturePolicy::default();
        let scope = RestoreScope {
            from,
            paths: &paths,
            root_path: &self.root,
            policy: &policy,
            naming: destination_naming(&self.root),
            renamed: &renamed,
            link_names: None,
        };
        let preflight = self.store.restore_preflight(&scope).expect("a preflight");
        let planned = plan(
            preflight.found.clone(),
            scope.naming,
            confirmed,
            resolutions,
            false,
        );
        let written = planned
            .proceeds()
            .then(|| self.store.restore_write(&scope, &preflight, &planned));

        (planned, written)
    }
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("its mode");
}

fn walk_directories(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root).expect("a directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            found.extend(walk_directories(&path));
            found.push(path);
        }
    }
    found
}

#[test]
fn a_mode_unlike_its_kind_s_default_is_one_record_and_the_rest_cost_nothing() {
    let fixture = Fixture::new(&[
        ("notes.txt", 0o644),
        ("run.sh", 0o755),
        ("docs/guide.md", 0o644),
        ("private/key", 0o600),
        ("bin/tool", 0o4755),
    ]);
    chmod(&fixture.root.join("private"), 0o700);

    fixture.capture(Trigger::Manual, MOMENT);

    assert_eq!(
        fixture.sidecar().as_deref(),
        Some(
            "keepster-meta 1 file=0644 dir=0755 modes\n\
             m 4755 bin/tool\n\
             m 0700 private\n\
             m 0600 private/key\n"
        ),
        "an executable file with `x` where it has `r` is no deviation"
    );
}

#[test]
fn a_permission_only_change_is_a_change() {
    let fixture = Fixture::new(&[("notes.txt", 0o644), ("key", 0o644)]);
    assert!(fixture.pass(MOMENT).snapshot.is_some());
    assert!(
        fixture.pass(MOMENT + 60).snapshot.is_none(),
        "nothing changed"
    );

    chmod(&fixture.root.join("key"), 0o600);
    let changed = fixture.pass(MOMENT + 120);

    let snapshot = changed.snapshot.expect("a Snapshot for the mode alone");
    assert_eq!(snapshot.changed_paths, Ok(1));
}

#[test]
fn a_reconciliation_takes_the_modes_most_paths_have_as_the_defaults() {
    let files: Vec<(String, u32)> = (0..20).map(|n| (format!("f{n}.txt"), 0o644)).collect();
    let borrowed: Vec<(&str, u32)> = files
        .iter()
        .map(|(path, mode)| (path.as_str(), *mode))
        .collect();
    let fixture = Fixture::new(&borrowed);
    fixture.capture(Trigger::Manual, MOMENT);

    // `chmod -R g+w .`
    for (path, _) in &files {
        chmod(&fixture.root.join(path), 0o664);
    }
    fixture.capture(Trigger::Manual, MOMENT + 60);
    assert_eq!(
        fixture.sidecar().map(|text| text.lines().count()),
        Some(21),
        "until a Reconciliation, the header stands and every path deviates from it"
    );
    assert!(
        fixture
            .pass_as(Trigger::Reconcile, MOMENT + 90)
            .snapshot
            .is_none(),
        "counting the header again changes no path, so it waits for a Snapshot that does"
    );

    fixture.capture(Trigger::Reconcile, MOMENT + 120);
    assert_eq!(
        fixture.sidecar().as_deref(),
        Some("keepster-meta 1 file=0664 dir=0755 modes\n"),
        "one header, not a record per path"
    );
}

#[test]
fn entries_carry_each_path_s_full_mode() {
    let fixture = Fixture::new(&[("notes.txt", 0o644), ("key", 0o600), ("run.sh", 0o755)]);
    let captured = fixture.capture(Trigger::Manual, MOMENT);

    let listing = fixture
        .store
        .entries(captured.id, &RootPath::root(), Depth::One)
        .expect("a listing");

    assert_eq!(
        listing
            .entries
            .iter()
            .map(|entry| (entry.path.as_str().to_owned(), entry.permissions))
            .collect::<Vec<_>>(),
        [
            ("key".to_owned(), Some(0o600)),
            ("notes.txt".to_owned(), Some(0o644)),
            ("run.sh".to_owned(), Some(0o755)),
        ]
    );
}

#[test]
fn a_diff_shows_a_change_of_mode_alone() {
    let fixture = Fixture::new(&[("notes.txt", 0o644), ("key", 0o644)]);
    let before = fixture.capture(Trigger::Manual, MOMENT);
    chmod(&fixture.root.join("key"), 0o600);
    let after = fixture.capture(Trigger::Manual, MOMENT + 60);

    let comparison = fixture
        .store
        .diff(before.id, &DiffTarget::Snapshot(after.id), None)
        .expect("a diff");

    assert_eq!(comparison.files.len(), 1, "{:?}", comparison.files);
    let file = &comparison.files[0];
    assert_eq!(file.path, rp("key"));
    assert_eq!(file.change, Some(FileChange::Modified));
    assert_eq!((file.old_mode, file.new_mode), (Some(0o644), Some(0o600)));

    let patch = fixture
        .store
        .unified(before.id, &DiffTarget::Snapshot(after.id), None)
        .expect("a patch");
    assert_eq!(
        String::from_utf8(patch).expect("text"),
        "diff --git a/key b/key\nold mode 100644\nnew mode 100600\n",
        "git's own lines, with every bit where git writes only the executable one"
    );
}

#[test]
fn diverging_modes_are_asked_about_and_applied_as_answered() {
    let fixture = Fixture::new(&[("key", 0o600), ("edited", 0o600), ("gone", 0o600)]);
    let saved = fixture.capture(Trigger::Manual, MOMENT);
    chmod(&fixture.root.join("key"), 0o644);
    fs::write(fixture.root.join("edited"), b"since").expect("edited");
    chmod(&fixture.root.join("edited"), 0o644);
    fs::remove_file(fixture.root.join("gone")).expect("deleted");

    let (asked, _) = fixture.restore(saved.id, &[], &Confirmed::all(), &BTreeMap::new());
    assert_eq!(
        asked
            .paths
            .iter()
            .map(|path| (path.path.as_str(), path.step, path.issues.clone()))
            .collect::<Vec<_>>(),
        [
            (
                "edited",
                Step::Blocked,
                vec![Issue::ModesDiffer {
                    snapshot: 0o600,
                    current: 0o644
                }]
            ),
            ("gone", Step::Create, vec![]),
            (
                "key",
                Step::Blocked,
                vec![Issue::ModesDiffer {
                    snapshot: 0o600,
                    current: 0o644
                }]
            ),
        ]
    );

    let (_, written) = fixture.restore(
        saved.id,
        &[],
        &Confirmed::none().with_modes(ModeChoice::Snapshot),
        &BTreeMap::from([(rp("edited"), Resolution::Modes(ModeChoice::Keep))]),
    );
    assert_eq!(
        written.expect("written"),
        [Written::Restored, Written::Created, Written::Restored]
    );
    assert_eq!(
        fixture.mode("key"),
        0o600,
        "the Snapshot's mode, applied alone"
    );
    assert_eq!(
        fs::read(fixture.root.join("edited")).expect("restored"),
        b"content"
    );
    assert_eq!(fixture.mode("edited"), 0o644, "content restored, mode kept");
    assert_eq!(
        fixture.mode("gone"),
        0o600,
        "a created file takes the Snapshot's mode, not the umask"
    );
}

#[test]
fn a_directory_whose_mode_alone_changed_is_in_the_diff() {
    let fixture = Fixture::new(&[("src/lib.rs", 0o644)]);
    let before = fixture.capture(Trigger::Manual, MOMENT);
    chmod(&fixture.root.join("src"), 0o700);
    let after = fixture.capture(Trigger::Manual, MOMENT + 60);
    assert_eq!(after.changed_paths, Ok(1));

    let comparison = fixture
        .store
        .diff(before.id, &DiffTarget::Snapshot(after.id), None)
        .expect("a diff");

    assert_eq!(comparison.files.len(), 1, "{:?}", comparison.files);
    assert_eq!(comparison.files[0].path, rp("src"));
    assert_eq!(
        (comparison.files[0].old_mode, comparison.files[0].new_mode),
        (Some(0o755), Some(0o700)),
        "what the timeline counts, the diff shows"
    );
}

#[test]
fn a_snapshot_that_observed_no_modes_makes_none_up() {
    let mut fixture = Fixture::new(&[("key", 0o600), ("run.sh", 0o755)]);
    let saved = fixture.capture(Trigger::Manual, MOMENT);
    fixture.as_if_no_modes_were_observed();

    let listing = fixture
        .store
        .entries(saved.id, &RootPath::root(), Depth::One)
        .expect("a listing");
    assert!(
        listing
            .entries
            .iter()
            .all(|entry| entry.permissions.is_none()),
        "{listing:?}"
    );

    chmod(&fixture.root.join("key"), 0o644);
    fs::write(fixture.root.join("run.sh"), b"since").expect("edited");
    chmod(&fixture.root.join("run.sh"), 0o700);
    let (planned, written) = fixture.restore(saved.id, &[], &Confirmed::none(), &BTreeMap::new());

    assert!(
        planned.paths.iter().all(|path| path.issues.is_empty()),
        "no mode to differ from: {planned:?}"
    );
    assert_eq!(
        written.expect("written"),
        [Written::Unchanged, Written::Restored]
    );
    assert_eq!(
        fixture.mode("key"),
        0o644,
        "never loosened to a made-up default"
    );
    assert_eq!(
        fixture.mode("run.sh"),
        0o700,
        "its own bits, with the Snapshot's executable bit"
    );
}
