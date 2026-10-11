//! Symlinks in a Snapshot (ticket 49): stored as links by default, followed where the follow list says,
//! never followed into a cycle, another Root or a History Store, and restored as links.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use std::collections::BTreeMap;

use keepster_core::{
    CapturePolicy, Confirmation, Confirmed, Incomplete, Issue, Level, Plan, RootId, RootPath, Rule,
    RuleOrigin, RuleSet, SnapshotId, Step, Trigger, WithheldReason, plan,
};
use keepster_store::{
    CapturedSnapshot, Depth, EntryKind, HistoryStore, IncompletePath, RestoreScope, Written,
    destination_naming,
};

/// 2026-09-30T14:37:05Z.
const MOMENT: u64 = 1_790_779_025;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
}

fn following(patterns: &[&str]) -> CapturePolicy {
    CapturePolicy::default().following(RuleSet::new(
        patterns
            .iter()
            .map(|pattern| {
                Rule::parse(
                    pattern,
                    RootPath::root(),
                    RuleOrigin {
                        level: Level::Global,
                        source: "test".to_owned(),
                        line: None,
                    },
                )
                .expect("a pattern")
            })
            .collect(),
    ))
}

struct Fixture {
    directory: tempfile::TempDir,
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
            directory,
            root,
            store,
            id: RootId::generate(at(MOMENT)),
        }
    }

    /// A directory beside the Root, outside it.
    fn outside(&self, name: &str) -> PathBuf {
        let path = self.directory.path().join(name);
        fs::create_dir_all(&path).expect("a directory outside the Root");
        path
    }

    fn write(&self, path: &str, content: &[u8]) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().expect("a parent")).expect("its directory");
        fs::write(path, content).expect("a file");
    }

    fn capture(&self, policy: &CapturePolicy, seconds: u64) -> CapturedSnapshot {
        self.store
            .capture(self.id, &self.root, policy, Trigger::Manual, at(seconds))
            .expect("a Snapshot")
    }

    /// The mode and content the newest Snapshot holds at `path`, read through git as a test may.
    fn held(&self, path: &str) -> Option<(i32, Vec<u8>)> {
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
        let entry = tree.get_path(Path::new(path)).ok()?;
        let blob = repository.find_blob(entry.id()).ok()?;
        Some((entry.filemode(), blob.content().to_vec()))
    }
}

impl Fixture {
    /// Plan a Restore of `paths` from `from`, and carry it out if nothing is blocked.
    fn restore(
        &self,
        from: SnapshotId,
        paths: &[&str],
        policy: &CapturePolicy,
        confirmed: &Confirmed,
    ) -> (Plan, Option<Vec<Written>>) {
        let paths: Vec<RootPath> = paths.iter().map(|path| rp(path)).collect();
        let renamed = BTreeMap::new();
        let scope = RestoreScope {
            from,
            paths: &paths,
            root_path: &self.root,
            policy,
            naming: destination_naming(&self.root),
            renamed: &renamed,
            link_names: None,
        };
        let preflight = self.store.restore_preflight(&scope).expect("a preflight");
        let planned = plan(
            preflight.found.clone(),
            scope.naming,
            confirmed,
            &BTreeMap::new(),
            false,
        );
        let written = planned
            .proceeds()
            .then(|| self.store.restore_write(&scope, &preflight, &planned));

        (planned, written)
    }
}

const LINK: i32 = 0o120_000;
const FILE: i32 = 0o100_644;

#[test]
fn a_link_is_stored_as_a_link_with_its_target_text_and_never_followed_by_default() {
    let fixture = Fixture::new();
    let elsewhere = fixture.outside("elsewhere");
    fs::write(elsewhere.join("secret.txt"), b"outside").expect("a file outside");
    symlink(&elsewhere, fixture.root.join("docs")).expect("a link to a directory");
    symlink("notes.txt", fixture.root.join("readme")).expect("a relative link");
    fixture.write("notes.txt", b"notes");

    let captured = fixture.capture(&CapturePolicy::default(), MOMENT);

    assert_eq!(
        fixture.held("docs"),
        Some((LINK, elsewhere.as_os_str().as_encoded_bytes().to_vec()))
    );
    assert_eq!(fixture.held("readme"), Some((LINK, b"notes.txt".to_vec())));
    assert_eq!(fixture.held("docs/secret.txt"), None, "not followed");
    assert!(captured.incomplete.is_empty() && captured.withheld.is_empty());

    let listing = fixture
        .store
        .entries(captured.id, &RootPath::root(), Depth::One)
        .expect("a listing");
    let docs = listing
        .entries
        .iter()
        .find(|entry| entry.path == rp("docs"))
        .expect("the link is listed");
    assert_eq!(docs.kind, EntryKind::Symlink);
}

#[test]
fn a_followed_link_captures_its_target_at_the_logical_path_under_the_root_s_rules() {
    let fixture = Fixture::new();
    let shared = fixture.outside("shared");
    fs::write(shared.join("plan.md"), b"plan").expect("a file");
    fs::write(shared.join("id_rsa"), b"key").expect("a private key");
    let note = fixture.outside("single");
    fs::write(note.join("note.txt"), b"one file").expect("a file");
    symlink(&shared, fixture.root.join("team")).expect("a link to a directory");
    symlink(note.join("note.txt"), fixture.root.join("note")).expect("a link to a file");

    let captured = fixture.capture(&following(&["**"]), MOMENT);

    assert_eq!(fixture.held("team/plan.md"), Some((FILE, b"plan".to_vec())));
    assert_eq!(
        fixture.held("team/id_rsa"),
        None,
        "the denylist judges the logical path, so a key reached through a link is still refused"
    );
    assert_eq!(fixture.held("note"), Some((FILE, b"one file".to_vec())));
    assert!(captured.incomplete.is_empty(), "{captured:?}");
}

#[test]
fn following_stops_at_a_cycle_and_at_another_root_storing_the_link_instead() {
    let fixture = Fixture::new();
    fixture.write("sub/a.txt", b"a");
    symlink(&fixture.root, fixture.root.join("sub/loop")).expect("a link back to the Root");
    let other = fixture.outside("other-root");
    fs::write(other.join("theirs.txt"), b"theirs").expect("a file of another Root");
    symlink(&other, fixture.root.join("other")).expect("a link into another Root");
    let policy = following(&["**"]).with_real_fences(vec![(
        other.canonicalize().expect("real"),
        WithheldReason::OtherRoot,
    )]);

    let captured = fixture.capture(&policy, MOMENT);

    assert_eq!(fixture.held("sub/loop").map(|(mode, _)| mode), Some(LINK));
    assert_eq!(fixture.held("other").map(|(mode, _)| mode), Some(LINK));
    assert_eq!(fixture.held("other/theirs.txt"), None);
    assert!(captured.incomplete.is_empty(), "{captured:?}");
}

#[test]
fn a_followed_target_containing_a_history_store_is_followed_with_the_store_withheld() {
    let fixture = Fixture::new();
    let data = fixture.outside("data");
    fs::write(data.join("kept.txt"), b"kept").expect("a file");
    fs::create_dir_all(data.join("keepster/1")).expect("a store");
    fs::write(data.join("keepster/1/HEAD"), b"ref").expect("store content");
    symlink(&data, fixture.root.join("data")).expect("a link");
    let policy = following(&["**"]).with_real_fences(vec![(
        data.join("keepster").canonicalize().expect("real"),
        WithheldReason::HistoryStore,
    )]);

    let captured = fixture.capture(&policy, MOMENT);

    assert_eq!(
        fixture.held("data/kept.txt"),
        Some((FILE, b"kept".to_vec()))
    );
    assert_eq!(fixture.held("data/keepster/1/HEAD"), None);
    assert!(
        captured
            .withheld
            .iter()
            .any(|withheld| withheld.path == rp("data/keepster")
                && withheld.reason == WithheldReason::HistoryStore),
        "{captured:?}"
    );
}

#[test]
fn an_unavailable_target_keeps_the_subtree_and_names_the_link() {
    let fixture = Fixture::new();
    let mount = fixture.outside("mount");
    fs::write(mount.join("photo.jpg"), b"photo").expect("a file");
    symlink(&mount, fixture.root.join("photos")).expect("a link");
    let policy = following(&["photos"]);
    fixture.capture(&policy, MOMENT);

    fs::remove_dir_all(&mount).expect("the target goes away, as an unmount does");
    let captured = fixture.capture(&policy, MOMENT + 60);

    assert_eq!(
        fixture.held("photos/photo.jpg"),
        Some((FILE, b"photo".to_vec())),
        "not thousands of deletions"
    );
    assert_eq!(
        captured.incomplete,
        [IncompletePath {
            path: rp("photos"),
            why: Incomplete::Error("enoent".to_owned()),
        }]
    );
}

#[test]
fn a_link_is_restored_as_a_link() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", b"notes");
    symlink("notes.txt", fixture.root.join("readme")).expect("a link");
    symlink("elsewhere", fixture.root.join("moved")).expect("a second link");
    let saved = fixture.capture(&CapturePolicy::default(), MOMENT);

    fs::remove_file(fixture.root.join("readme")).expect("deleted");
    fs::remove_file(fixture.root.join("moved")).expect("retargeted");
    symlink("somewhere-else", fixture.root.join("moved")).expect("retargeted");
    let (planned, written) = fixture.restore(
        saved.id,
        &["readme", "moved"],
        &CapturePolicy::default(),
        &Confirmed::none(),
    );

    assert!(planned.proceeds(), "{planned:?}");
    assert_eq!(
        written.expect("written"),
        [Written::Restored, Written::Created],
        "a link over a link is replaced in place, and a missing one created"
    );
    assert_eq!(
        fs::read_link(fixture.root.join("readme")).expect("a link"),
        Path::new("notes.txt")
    );
    assert_eq!(
        fs::read_link(fixture.root.join("moved")).expect("a link"),
        Path::new("elsewhere")
    );
}

#[test]
fn a_file_where_the_snapshot_holds_a_link_is_a_change_of_type() {
    let fixture = Fixture::new();
    symlink("target", fixture.root.join("entry")).expect("a link");
    let saved = fixture.capture(&CapturePolicy::default(), MOMENT);
    fs::remove_file(fixture.root.join("entry")).expect("deleted");
    fixture.write("entry", b"a file now");

    let (refused, _) = fixture.restore(
        saved.id,
        &["entry"],
        &CapturePolicy::default(),
        &Confirmed::none(),
    );
    assert!(matches!(
        refused.paths[0].issues[..],
        [Issue::TypeChange { .. }]
    ));

    let (_, written) = fixture.restore(
        saved.id,
        &["entry"],
        &CapturePolicy::default(),
        &Confirmed::these([Confirmation::ReplaceType]),
    );
    assert_eq!(written.expect("written"), [Written::Restored]);
    assert_eq!(
        fs::read_link(fixture.root.join("entry")).expect("a link again"),
        Path::new("target")
    );
}

#[test]
fn a_restore_writes_through_a_followed_link_only_while_it_is_followed() {
    let fixture = Fixture::new();
    let shared = fixture.outside("shared");
    fs::write(shared.join("plan.md"), b"saved").expect("a file");
    fs::write(shared.join("solo.txt"), b"solo saved").expect("a file");
    symlink(&shared, fixture.root.join("team")).expect("a link to a directory");
    symlink(shared.join("solo.txt"), fixture.root.join("solo")).expect("a link to a file");
    let policy = following(&["**"]);
    let saved = fixture.capture(&policy, MOMENT);
    fs::write(shared.join("plan.md"), b"edited").expect("edited");
    fs::write(shared.join("solo.txt"), b"solo edited").expect("edited");

    let (refused, _) = fixture.restore(
        saved.id,
        &["team/plan.md"],
        &CapturePolicy::default(),
        &Confirmed::none(),
    );
    assert_eq!(refused.paths[0].step, Step::Blocked);
    assert!(
        matches!(refused.paths[0].issues[..], [Issue::LinkCheckFailed { .. }]),
        "a link no longer followed is not written through: {refused:?}"
    );

    let (planned, written) = fixture.restore(
        saved.id,
        &["team/plan.md", "solo"],
        &policy,
        &Confirmed::none(),
    );
    assert!(planned.proceeds(), "{planned:?}");
    assert_eq!(
        written.expect("written"),
        [Written::Restored, Written::Restored]
    );
    assert_eq!(
        fs::read(shared.join("plan.md")).expect("the target"),
        b"saved"
    );
    assert_eq!(
        fs::read(shared.join("solo.txt")).expect("the target"),
        b"solo saved"
    );
    assert!(
        fs::symlink_metadata(fixture.root.join("team"))
            .expect("the link")
            .file_type()
            .is_symlink()
            && fs::symlink_metadata(fixture.root.join("solo"))
                .expect("the link")
                .file_type()
                .is_symlink(),
        "both links are still links"
    );
}

#[test]
fn a_chain_of_links_back_into_a_followed_tree_is_a_cycle_too() {
    // `a -> p`, `p/x/b -> q`, `q/c -> p/x`: no target holds another by path, but `p/x` is already on
    // the walk by its identity.
    let fixture = Fixture::new();
    let p = fixture.outside("p");
    let q = fixture.outside("q");
    fs::create_dir_all(p.join("x")).expect("a directory");
    fs::write(p.join("x/once.txt"), b"once").expect("a file");
    symlink(&p, fixture.root.join("a")).expect("a link");
    symlink(&q, p.join("x/b")).expect("a link");
    symlink(p.join("x"), q.join("c")).expect("a link back");

    let captured = fixture.capture(&following(&["**"]), MOMENT);

    assert_eq!(fixture.held("a/x/once.txt"), Some((FILE, b"once".to_vec())));
    assert_eq!(
        fixture.held("a/x/b/c").map(|(mode, _)| mode),
        Some(LINK),
        "the link that closes the cycle is stored as a link"
    );
    assert!(captured.incomplete.is_empty(), "{captured:?}");
}

#[test]
fn an_unmounted_target_named_by_a_directory_pattern_still_keeps_its_subtree() {
    let fixture = Fixture::new();
    let mount = fixture.outside("mount");
    fs::write(mount.join("photo.jpg"), b"photo").expect("a file");
    symlink(&mount, fixture.root.join("photos")).expect("a link");
    let policy = following(&["photos/"]);
    fixture.capture(&policy, MOMENT);

    fs::remove_dir_all(&mount).expect("unmounted");
    let captured = fixture.capture(&policy, MOMENT + 60);

    assert_eq!(
        fixture.held("photos/photo.jpg"),
        Some((FILE, b"photo".to_vec()))
    );
    assert_eq!(captured.incomplete.len(), 1, "{captured:?}");
}

#[test]
fn a_link_retargeted_at_a_lookalike_tree_is_read_again() {
    // Same names, same sizes, same mtimes: only where the files really are tells them apart.
    let fixture = Fixture::new();
    let v1 = fixture.outside("v1");
    let v2 = fixture.outside("v2");
    // An hour before the passes, so the first pass's evidence is old enough to vouch next time.
    for version in [&v1, &v2] {
        fs::write(version.join("app.cfg"), b"version ?").expect("a file");
        fs::File::options()
            .write(true)
            .open(version.join("app.cfg"))
            .expect("the file")
            .set_modified(at(MOMENT - 3_600))
            .expect("an old mtime");
    }
    fs::write(v1.join("app.cfg"), b"version 1").expect("first");
    fs::write(v2.join("app.cfg"), b"version 2").expect("second");
    for version in [&v1, &v2] {
        fs::File::options()
            .write(true)
            .open(version.join("app.cfg"))
            .expect("the file")
            .set_modified(at(MOMENT - 3_600))
            .expect("the same old mtime");
    }
    symlink(&v1, fixture.root.join("current")).expect("a link");
    let policy = following(&["current"]);
    fixture.capture(&policy, MOMENT);
    fs::remove_file(fixture.root.join("current")).expect("unlinked");
    symlink(&v2, fixture.root.join("current")).expect("retargeted");

    fixture.capture(&policy, MOMENT + 3_600);

    assert_eq!(
        fixture.held("current/app.cfg"),
        Some((FILE, b"version 2".to_vec()))
    );
}

#[test]
fn a_restore_never_writes_through_a_link_into_another_root() {
    let fixture = Fixture::new();
    let shared = fixture.outside("shared");
    fs::create_dir_all(shared.join("theirs")).expect("a directory");
    fs::write(shared.join("theirs/f.txt"), b"saved").expect("a file");
    symlink(&shared, fixture.root.join("team")).expect("a link");
    let saved = fixture.capture(&following(&["**"]), MOMENT);

    // Since the Snapshot, `shared/theirs` became another Root.
    let policy = following(&["**"]).with_real_fences(vec![(
        shared.join("theirs").canonicalize().expect("real"),
        WithheldReason::OtherRoot,
    )]);
    let (refused, _) =
        fixture.restore(saved.id, &["team/theirs/f.txt"], &policy, &Confirmed::all());

    assert_eq!(refused.paths[0].step, Step::Blocked);
    assert_eq!(
        refused.paths[0].issues,
        [Issue::LinkCheckFailed { at: rp("team") }],
        "the link's own target is allowed, but the write would land in another Root"
    );
}
