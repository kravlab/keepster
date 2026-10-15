//! The content format against a real store (ticket 55, ADR 0010): there is no migration. Every commit
//! records the format it was written in, a Segment from before the format was recorded is read as the
//! first, and a Segment in a format newer than this build is opaque — never read, never rewritten —
//! while capture carries on beside it in a Segment of this build's own.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use keepster_core::{
    CapturePolicy, ContentFormat, Ladder, Level, Newest, RootId, RootPath, Rule, RuleOrigin,
    RuleSet, SegmentName, SnapshotId, Trigger,
};
use keepster_store::{HistoryStore, SnapshotSummary, StoreError, TimelinePage, TimelineQuery};

/// 2026-09-30T14:00:00Z.
const NOW: u64 = 1_790_776_800;
const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn path(text: &str) -> RootPath {
    RootPath::parse(text).expect("a path")
}

fn segment_ref(name: &str) -> String {
    format!("refs/keepster/segments/{name}")
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
        let store = Self::open(&directory);

        Self {
            directory,
            root,
            store,
            id: RootId::generate(at(NOW)),
        }
    }

    fn open(directory: &tempfile::TempDir) -> HistoryStore {
        HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store")
    }

    /// The same store with its index deleted, for the next read to rebuild — closed first, since
    /// Windows deletes no file a connection holds open.
    fn with_index_rebuilt(self) -> Self {
        let Self {
            directory,
            root,
            store,
            id,
        } = self;
        drop(store);
        fs::remove_file(directory.path().join("index.sqlite")).expect("the index deleted");
        let store = Self::open(&directory);

        Self {
            directory,
            root,
            store,
            id,
        }
    }

    fn write(&self, name: &str, content: &str) {
        fs::write(self.root.join(name), content).expect("a file");
    }

    fn capture(&self, moment: u64) -> keepster_store::CapturedSnapshot {
        let policy =
            CapturePolicy::new(RuleSet::default(), RuleSet::default(), 1 << 20, Vec::new());
        self.store
            .capture(self.id, &self.root, &policy, Trigger::Manual, at(moment))
            .expect("a Snapshot")
    }

    fn page(&self) -> TimelinePage {
        self.store
            .timeline(&TimelineQuery {
                path: None,
                before: None,
                not_before: None,
                limit: 1000,
            })
            .expect("a timeline")
    }

    fn timeline(&self) -> Vec<SnapshotSummary> {
        self.page().snapshots
    }

    fn repository(&self) -> git2::Repository {
        git2::Repository::open_bare(self.directory.path().join("history.git")).expect("the store")
    }

    fn tip(&self, segment: &str) -> Option<git2::Oid> {
        self.repository()
            .find_reference(&segment_ref(segment))
            .ok()
            .and_then(|reference| reference.target())
    }

    /// A Segment as a build of format 2 might write it in the hour of `moment`, under `name`: two
    /// Snapshots holding `future.txt`, the format in its root commit only — where ADR 0010 puts it —
    /// and otherwise every trailer this build would read, so only its format keeps it out.
    fn newer_segment(&self, name: &str, moment: u64) -> Vec<SnapshotId> {
        let repository = self.repository();
        let blob = repository.blob(b"from the future").expect("a blob");
        let mut builder = repository.treebuilder(None).expect("a tree");
        builder
            .insert("future.txt", blob, 0o100_644)
            .expect("an entry");
        let tree = repository
            .find_tree(builder.write().expect("written"))
            .expect("the tree");
        let signature = git2::Signature::new(
            "Keepster",
            "keepster@localhost",
            &git2::Time::new(i64::try_from(moment).expect("a moment"), 0),
        )
        .expect("a signature");

        let mut ids = Vec::new();
        let mut parent: Option<git2::Commit<'_>> = None;
        for (offset, format) in [(1, "Keepster-Format: 2\n"), (2, "")] {
            let id = SnapshotId::following(ids.last().copied(), at(moment + offset));
            let message = format!(
                "Snapshot\n\nKeepster-Snapshot: {id}\nKeepster-Root: {}\nKeepster-Trigger: manual\n\
                 Keepster-Omitted: 0\n{format}",
                self.id
            );
            let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
            let commit = repository
                .commit(None, &signature, &signature, &message, &tree, &parents)
                .expect("a commit");
            parent = Some(repository.find_commit(commit).expect("the commit"));
            ids.push(id);
        }
        repository
            .reference(
                &segment_ref(name),
                parent.expect("two commits").id(),
                true,
                "a newer build",
            )
            .expect("its ref");

        ids
    }
}

#[test]
fn every_commit_records_the_format_it_was_written_in() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", "one");
    fixture.capture(NOW + 60);
    fixture.write("notes.txt", "two");
    let second = fixture.capture(NOW + 120);
    fixture.store.pin(second.id).expect("pinned");
    let repository = fixture.repository();

    let expected = format!("Keepster-Format: {}", ContentFormat::CURRENT);
    let tip = repository
        .find_commit(fixture.tip("2026-09-30T14").expect("the Segment"))
        .expect("its tip");
    let root = tip.parent(0).expect("its root");
    let pin = repository
        .find_reference(&format!("refs/keepster/pins/{}", second.id))
        .and_then(|reference| reference.peel_to_commit())
        .expect("the Pin");
    for commit in [&root, &tip, &pin] {
        assert!(
            commit.message().unwrap_or_default().contains(&expected),
            "so a Segment thinned of its root, and a Pin on its own, still say it: {:?}",
            commit.message()
        );
    }
}

#[test]
fn a_segment_from_before_the_format_was_recorded_is_read_as_the_first() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", "old");
    let old = fixture.capture(NOW - HOUR + 60);
    // Rewritten as a build from before ticket 55 wrote it: without the trailer.
    {
        let repository = fixture.repository();
        let commit = repository
            .find_commit(fixture.tip("2026-09-30T13").expect("the Segment"))
            .expect("its commit");
        let message: String = commit
            .message()
            .expect("UTF-8")
            .lines()
            .filter(|line| !line.starts_with("Keepster-Format"))
            .map(|line| format!("{line}\n"))
            .collect();
        let legacy = repository
            .commit(
                None,
                &commit.author(),
                &commit.committer(),
                &message,
                &commit.tree().expect("its tree"),
                &[],
            )
            .expect("a legacy commit");
        repository
            .reference(&segment_ref("2026-09-30T13"), legacy, true, "legacy")
            .expect("moved");
    }
    let fixture = fixture.with_index_rebuilt();
    fixture.write("notes.txt", "new");

    let new = fixture.capture(NOW + 60);

    assert_eq!(new.segment.as_str(), "2026-09-30T14");
    assert_eq!(
        new.changed_paths,
        Ok(1),
        "its baseline is the legacy Segment's Snapshot"
    );
    let timeline: Vec<SnapshotId> = fixture.timeline().iter().map(|s| s.id).collect();
    assert_eq!(timeline, vec![new.id, old.id], "both formats read");
    let content = fixture
        .store
        .content(old.id, &path("notes.txt"))
        .expect("readable");
    assert_eq!(content.size(), 3);
}

#[test]
fn a_segment_newer_than_this_build_is_opaque_and_capture_carries_on_beside_it() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", "before");
    let before = fixture.capture(NOW - HOUR + 60);
    let future = fixture.newer_segment("2026-09-30T14", NOW + 10);
    let future_tip = fixture.tip("2026-09-30T14");
    fixture.write("notes.txt", "after");

    let first = fixture.capture(NOW + 60);
    fixture.write("notes.txt", "later");
    let second = fixture.capture(NOW + 120);

    let sibling = SegmentName::hourly(at(NOW))
        .expect("a name")
        .in_format(ContentFormat::CURRENT);
    assert_eq!(first.segment, sibling, "a Segment of its own in this hour");
    assert_eq!(second.segment, sibling, "and kept for the rest of it");
    assert_eq!(
        first.changed_paths,
        Ok(1),
        "its baseline is the newest Snapshot this build can read, not the opaque one"
    );
    let repository = fixture.repository();
    let tip = repository
        .find_commit(fixture.tip(sibling.as_str()).expect("the sibling"))
        .expect("its tip");
    assert_eq!(tip.parent_count(), 1);
    assert_eq!(
        tip.parent(0).expect("its root").parent_count(),
        0,
        "the sibling starts its own chain"
    );
    assert_eq!(
        fixture.tip("2026-09-30T14"),
        future_tip,
        "the newer Segment is never written to"
    );

    let page = fixture.page();
    let timeline: Vec<SnapshotId> = page.snapshots.iter().map(|s| s.id).collect();
    assert_eq!(
        timeline,
        vec![second.id, first.id, before.id],
        "the newer Segment's Snapshots are opaque, not misread"
    );
    assert_eq!(
        page.unreadable,
        vec!["2026-09-30T14".to_owned()],
        "and reported where they would have been listed"
    );
    let fixture = fixture.with_index_rebuilt();
    assert_eq!(
        fixture.page().unreadable,
        page.unreadable,
        "after a rebuild too"
    );
    for id in future {
        assert!(
            matches!(
                fixture.store.content(id, &path("future.txt")),
                Err(StoreError::SnapshotNotFound(_))
            ),
            "{id}"
        );
    }
}

#[test]
fn a_newer_sibling_leaves_this_build_appending_to_its_own_segment() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", "one");
    let first = fixture.capture(NOW + 60);
    let hour = SegmentName::hourly(at(NOW)).expect("a name");
    fixture.newer_segment(hour.in_format(ContentFormat::new(2)).as_str(), NOW + 90);
    fixture.write("notes.txt", "two");

    let second = fixture.capture(NOW + 120);

    assert_eq!(second.segment, hour);
    let repository = fixture.repository();
    let tip = repository
        .find_commit(fixture.tip(hour.as_str()).expect("the Segment"))
        .expect("its tip");
    assert_eq!(
        tip.parent_count(),
        1,
        "appended to the Segment of its format"
    );
    assert_eq!(
        fixture.timeline().iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![second.id, first.id]
    );
}

#[test]
fn nothing_rewrites_a_segment_newer_than_this_build() {
    let fixture = Fixture::new();
    fixture.write("future.txt", "mine");
    fixture.capture(NOW - 2 * DAY);
    fixture.newer_segment("2026-09-28T10", NOW - 2 * DAY + 600);
    let future_tip = fixture.tip("2026-09-28T10");

    let forgotten = fixture
        .store
        .forget(
            &Rule::parse(
                "future.txt",
                RootPath::root(),
                RuleOrigin {
                    level: Level::Forget,
                    source: "keepster forget".to_owned(),
                    line: None,
                },
            )
            .expect("a pattern"),
            None,
            &mut |_, _| {},
        )
        .expect("forgotten");
    assert_eq!(forgotten.snapshots_touched, 1, "its own Segment's");
    assert_eq!(
        forgotten.left_alone,
        vec![segment_ref("2026-09-28T10")],
        "named, since what it holds was not forgotten"
    );
    assert_eq!(fixture.tip("2026-09-28T10"), future_tip);

    // Far past every Tier, every Segment this build reads ages out; the newer one is not its to judge.
    let retained = fixture
        .store
        .retain(&Ladder::default(), at(NOW + 400 * DAY), Newest::Ages)
        .expect("retained");
    assert_eq!(retained.deleted_segments, 1, "{retained:?}");
    assert_eq!(fixture.tip("2026-09-28T10"), future_tip);
    let listed: BTreeSet<String> = fixture
        .store
        .segments()
        .expect("listed")
        .iter()
        .map(|segment| segment.name.to_string())
        .collect();
    assert!(listed.is_empty(), "Retention never sees it: {listed:?}");
}

#[test]
fn a_sidecar_record_this_build_does_not_know_is_skipped_and_reported_where_it_is_met() {
    // ADR 0010: the record-type alphabet only grows. A later build's record is skipped — the Snapshot
    // reads as it always did — and reported, both by a listing and by the pass that could not carry it.
    let fixture = Fixture::new();
    fixture.write("notes.txt", "kept");
    fixture.capture(NOW + 60);
    let later = {
        let repository = fixture.repository();
        let tip = repository
            .find_commit(fixture.tip("2026-09-30T14").expect("the Segment"))
            .expect("its tip");
        let sidecar = repository
            .blob(b"keepster-meta 1 file=0644 dir=0755\nz a_fact_from_the_future notes.txt\n")
            .expect("a sidecar");
        let mut builder = repository
            .treebuilder(Some(&tip.tree().expect("its tree")))
            .expect("a tree");
        builder
            .insert(".keepster-meta", sidecar, 0o100_644)
            .expect("the sidecar");
        let tree = repository
            .find_tree(builder.write().expect("written"))
            .expect("the tree");
        let rewritten = repository
            .commit(
                None,
                &tip.author(),
                &tip.committer(),
                tip.message().expect("UTF-8"),
                &tree,
                &[],
            )
            .expect("as a later build wrote it");
        repository
            .reference(&segment_ref("2026-09-30T14"), rewritten, true, "later")
            .expect("moved");
        tip.message()
            .ok()
            .and_then(|message| {
                message
                    .lines()
                    .find_map(|line| line.strip_prefix("Keepster-Snapshot: "))
            })
            .and_then(|id| id.parse::<SnapshotId>().ok())
            .expect("its identity")
    };

    let listing = fixture
        .store
        .entries(later, &RootPath::root(), keepster_store::Depth::One)
        .expect("a listing");
    assert_eq!(
        listing
            .entries
            .iter()
            .map(|entry| entry.path.to_lossy())
            .collect::<Vec<_>>(),
        ["notes.txt"],
        "read as it always was"
    );
    assert_eq!(listing.unrecognized, ["z a_fact_from_the_future notes.txt"]);

    let policy = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 1 << 20, Vec::new());
    let pass = fixture
        .store
        .pass(
            fixture.id,
            &fixture.root,
            &policy,
            &keepster_store::CaptureRequest {
                trigger: Trigger::Manual,
                now: at(NOW + 120),
                verify: keepster_core::Verify::Stat,
                record_unchanged: true,
                hold_emptied: false,
            },
        )
        .expect("a pass");
    assert_eq!(pass.unrecognized, ["z a_fact_from_the_future notes.txt"]);
    assert!(pass.snapshot.is_some(), "capture carries on");
}

#[test]
fn a_pin_newer_than_this_build_is_named_rather_than_listed() {
    let fixture = Fixture::new();
    fixture.write("notes.txt", "mine");
    let mine = fixture.capture(NOW + 60);
    fixture.store.pin(mine.id).expect("pinned");
    let future = fixture.newer_segment("2026-09-30T13", NOW - HOUR + 10);
    {
        // A newer build's Pin: the parentless first commit of its Segment, whose ref is gone.
        let repository = fixture.repository();
        let tip = repository
            .find_commit(fixture.tip("2026-09-30T13").expect("its Segment"))
            .expect("its tip");
        let root = tip.parent(0).expect("its root");
        repository
            .reference(
                &format!("refs/keepster/pins/{}", future[0]),
                root.id(),
                true,
                "a newer build's Pin",
            )
            .expect("pinned");
        repository
            .find_reference(&segment_ref("2026-09-30T13"))
            .and_then(|mut reference| reference.delete())
            .expect("its Segment gone");
    }

    let pins: Vec<SnapshotId> = fixture
        .store
        .pins()
        .expect("listed")
        .iter()
        .map(|pin| pin.id)
        .collect();

    assert_eq!(pins, vec![mine.id], "only what this build reads is listed");
    assert_eq!(
        fixture.store.unreadable().expect("named"),
        vec![format!("pins/{}", future[0])],
        "and the rest is named, not dropped in silence"
    );
}
