//! Retention against a real store (ticket 51): a month of history built through the capture path at
//! injected moments, then the ladder applied — Segments past a coarser Tier deleted, Segments within a
//! Tier as fine as them re-chained, every surviving Snapshot keeping its identity and its content, the
//! timeline counting changes against the new neighbours, and a second pass changing nothing.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use keepster_core::{CapturePolicy, Ladder, RootId, RootPath, SnapshotId, Trigger, plan_retention};
use keepster_store::{HistoryStore, TimelineQuery};

/// 2026-09-30T14:00:00Z.
const NOW: u64 = 1_790_776_800;
const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

struct Fixture {
    directory: tempfile::TempDir,
    root: PathBuf,
    store: HistoryStore,
    id: RootId,
    /// Every Snapshot taken, with the content `counter.txt` held in it.
    taken: Vec<(SnapshotId, String)>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = directory.path().join("project");
        fs::create_dir_all(&root).expect("the Root");
        fs::write(root.join("stable.txt"), b"never changes").expect("a file");
        let store = HistoryStore::open(
            directory.path().join("history.git"),
            directory.path().join("index.sqlite"),
        )
        .expect("a store");

        Self {
            directory,
            root,
            store,
            id: RootId::generate(at(NOW)),
            taken: Vec::new(),
        }
    }

    /// A Snapshot at `moment`, with `counter.txt` changed for it.
    fn capture(&mut self, moment: u64) {
        let content = format!("taken at {moment}");
        fs::write(self.root.join("counter.txt"), &content).expect("a change");
        let captured = self
            .store
            .capture(
                self.id,
                &self.root,
                &CapturePolicy::default(),
                Trigger::Manual,
                at(moment),
            )
            .expect("a Snapshot");
        self.taken.push((captured.id, content));
    }

    fn timeline(&self, path: Option<&str>) -> Vec<(SnapshotId, u64)> {
        let page = self
            .store
            .timeline(&TimelineQuery {
                path: path.map(|path| RootPath::parse(path).expect("a path")),
                before: None,
                not_before: None,
                limit: 100_000,
            })
            .expect("a timeline");
        page.snapshots
            .iter()
            .map(|snapshot| (snapshot.id, snapshot.changed_paths))
            .collect()
    }

    fn content(&self, snapshot: SnapshotId) -> String {
        let content = self
            .store
            .content(snapshot, &RootPath::parse("counter.txt").expect("a path"))
            .expect("the version");
        let mut bytes = Vec::new();
        content
            .read(0, content.size(), &mut |chunk| {
                bytes.extend_from_slice(chunk);
                Ok(())
            })
            .expect("read");
        String::from_utf8(bytes).expect("text")
    }
}

#[test]
fn a_month_of_history_goes_down_the_ladder() {
    let mut fixture = Fixture::new();
    // Every six hours for forty days, and twice an hour, twenty minutes apart, for the last eight.
    let mut hour = NOW - 40 * DAY;
    while hour < NOW {
        if hour >= NOW - 8 * DAY {
            fixture.capture(hour + 5 * 60);
            fixture.capture(hour + 25 * 60);
        } else if (NOW - hour).is_multiple_of(6 * HOUR) {
            fixture.capture(hour + 5 * 60);
        }
        hour += HOUR;
    }
    let ladder = Ladder::default();

    let retained = fixture.store.retain(&ladder, at(NOW)).expect("a pass");

    assert_eq!(retained.index_lag, None);
    let segments = fixture.store.segments().expect("Segments");
    let survivors: BTreeSet<SnapshotId> = segments
        .iter()
        .flat_map(|segment| segment.snapshots.iter().copied())
        .collect();
    let in_window = |from: u64, to: u64| {
        survivors
            .iter()
            .filter(|id| id.moment() >= at(from) && id.moment() < at(to))
            .count()
    };
    assert_eq!(in_window(NOW - DAY, NOW), 48, "the last day is whole");
    assert_eq!(
        in_window(NOW - 7 * DAY + HOUR, NOW - DAY),
        6 * 24 - 1,
        "one an hour to a week, below the hour still straddling the week's boundary"
    );
    assert_eq!(
        in_window(NOW - 7 * DAY, NOW - 7 * DAY + HOUR),
        2,
        "an hour a boundary runs through is left whole until it has crossed"
    );
    let days: BTreeSet<u64> = survivors
        .iter()
        .filter(|id| id.moment() >= at(NOW - 29 * DAY) && id.moment() < at(NOW - 8 * DAY))
        .map(|id| {
            id.moment()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("after the epoch")
                .as_secs()
                / DAY
        })
        .collect();
    assert_eq!(
        days.len(),
        in_window(NOW - 29 * DAY, NOW - 8 * DAY),
        "one a day to a month"
    );
    assert_eq!(in_window(0, NOW - 30 * DAY), 0, "nothing older");
    assert_eq!(
        retained.removed_snapshots,
        u64::try_from(fixture.taken.len() - survivors.len()).expect("a count")
    );

    // Each survivor is the Snapshot it was: the same identity, the same content.
    for (id, content) in fixture
        .taken
        .iter()
        .filter(|(id, _)| survivors.contains(id))
    {
        assert_eq!(&fixture.content(*id), content, "{id}");
    }
    // The timeline is the survivors, newest first, each counted against the one before it.
    let timeline = fixture.timeline(None);
    assert_eq!(
        timeline.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>(),
        survivors
    );
    assert_eq!(
        fixture.timeline(Some("counter.txt")).len(),
        survivors.len(),
        "every survivor changed the counter against its new neighbour"
    );
    let (oldest, rest) = timeline.split_last().expect("survivors");
    assert_eq!(oldest.1, 2, "the oldest survivor adds both files");
    assert!(
        rest.iter().all(|(_, changed)| *changed == 1),
        "only the counter, never the stable file"
    );

    let again = plan_retention(&segments, at(NOW), &ladder);
    assert!(again.is_empty(), "a second pass changes nothing: {again:?}");

    // The next capture still has its baseline.
    fixture.capture(NOW + 60);
    let newest = fixture.timeline(None)[0];
    assert_eq!(
        newest.1, 1,
        "counted against the Snapshot before it, not from nothing"
    );
    drop(fixture.directory);
}

#[test]
fn a_rebuilt_index_agrees_with_one_caught_up_after_thinning() {
    let mut fixture = Fixture::new();
    let mut moment = NOW - 3 * DAY;
    while moment < NOW {
        fixture.capture(moment);
        moment += 15 * 60;
    }
    fixture
        .store
        .retain(&Ladder::default(), at(NOW))
        .expect("a pass");
    let caught_up = fixture.timeline(None);

    let index = fixture.directory.path().join("index.sqlite");
    fs::remove_file(&index).expect("the index");
    let reopened = HistoryStore::open(fixture.directory.path().join("history.git"), &index)
        .expect("the store");
    let rebuilt = reopened
        .timeline(&TimelineQuery {
            path: None,
            before: None,
            not_before: None,
            limit: 100_000,
        })
        .expect("a timeline");

    assert_eq!(
        rebuilt
            .snapshots
            .iter()
            .map(|snapshot| (snapshot.id, snapshot.changed_paths))
            .collect::<Vec<_>>(),
        caught_up
    );
}

#[test]
fn a_segment_this_product_did_not_write_is_left_alone() {
    let mut fixture = Fixture::new();
    let old = NOW - 40 * DAY;
    fixture.capture(old + 60);
    fixture.capture(NOW - 60);
    // Someone commits into the old Segment by hand.
    let repository = git2::Repository::open_bare(fixture.directory.path().join("history.git"))
        .expect("the store");
    let name = format!(
        "refs/keepster/segments/{}",
        keepster_core::SegmentName::hourly(at(old)).expect("a name")
    );
    let tip = repository
        .find_reference(&name)
        .and_then(|reference| reference.peel_to_commit())
        .expect("the Segment");
    let signature = git2::Signature::new(
        "someone",
        "someone@example.com",
        &git2::Time::new(i64::try_from(NOW).expect("a moment"), 0),
    )
    .expect("a signature");
    let foreign = repository
        .commit(
            None,
            &signature,
            &signature,
            "by hand",
            &tip.tree().expect("a tree"),
            &[&tip],
        )
        .expect("a commit");
    repository
        .reference(&name, foreign, true, "by hand")
        .expect("moved");

    let retained = fixture
        .store
        .retain(&Ladder::default(), at(NOW))
        .expect("a pass");

    assert_eq!(retained.removed_snapshots, 0, "{retained:?}");
    assert!(repository.find_reference(&name).is_ok(), "still there");
}
