//! The Retention ladder as a pure plan (ticket 51, ticket 06): which Segments are deleted outright, which
//! are thinned to the last Snapshot of each period, and that a Segment straddling a Tier boundary is left
//! alone until it has crossed — so a second pass over the result changes nothing.

use std::time::{Duration, SystemTime};

use keepster_core::{
    InvalidLadder, Keep, Ladder, Newest, RetentionPlan, SegmentContents, SegmentName, SnapshotId,
    Thinning, Tier, plan_retention,
};

/// 2026-09-30T14:00:00Z.
const NOW: u64 = 1_790_776_800;
const HOUR: u64 = 3600;
const DAY: u64 = 24 * HOUR;

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
}

fn id(seconds: u64) -> SnapshotId {
    SnapshotId::following(None, at(seconds))
}

/// Hourly Segments holding Snapshots taken at these moments, as a capture would have filed them.
fn segments(moments: &[u64]) -> Vec<SegmentContents> {
    let mut found: Vec<SegmentContents> = Vec::new();
    for moment in moments {
        let snapshot = id(*moment);
        let name = SegmentName::hourly(at(*moment)).expect("a name");
        match found.iter_mut().find(|segment| segment.name == name) {
            Some(segment) => segment.snapshots.push(snapshot),
            None => found.push(SegmentContents {
                name,
                snapshots: vec![snapshot],
            }),
        }
    }
    for segment in &mut found {
        segment.snapshots.sort();
    }
    found
}

/// The store as it is once `plan` is applied.
fn applied(mut segments: Vec<SegmentContents>, plan: &RetentionPlan) -> Vec<SegmentContents> {
    segments.retain(|segment| !plan.delete.contains(&segment.name));
    for thinning in &plan.thin {
        let segment = segments
            .iter_mut()
            .find(|segment| segment.name == thinning.segment)
            .expect("a Segment the plan names");
        segment.snapshots = thinning.keep.clone();
    }
    segments
}

fn name(moment: u64) -> SegmentName {
    SegmentName::hourly(at(moment)).expect("a name")
}

#[test]
fn the_shipped_ladder_keeps_everything_a_day_then_hourly_to_a_week_then_daily_to_a_month() {
    assert_eq!(
        Ladder::default().tiers(),
        [
            Tier {
                age: Duration::from_secs(DAY),
                keep: Keep::All,
            },
            Tier {
                age: Duration::from_secs(7 * DAY),
                keep: Keep::Every(Duration::from_secs(HOUR)),
            },
            Tier {
                age: Duration::from_secs(30 * DAY),
                keep: Keep::Every(Duration::from_secs(DAY)),
            },
        ]
    );
}

#[test]
fn the_last_day_is_never_thinned() {
    let store = segments(&[
        NOW - 2 * HOUR,
        NOW - 2 * HOUR + 60,
        NOW - 20 * HOUR,
        NOW - 60,
    ]);

    assert_eq!(
        plan_retention(&store, at(NOW), &Ladder::default(), Newest::Survives),
        RetentionPlan::default()
    );
}

#[test]
fn an_hour_wholly_past_the_first_tier_keeps_its_last_snapshot() {
    let hour = NOW - 2 * DAY;
    let store = segments(&[hour + 60, hour + 120, hour + 1800, NOW - 60]);

    let plan = plan_retention(&store, at(NOW), &Ladder::default(), Newest::Survives);

    assert_eq!(
        plan,
        RetentionPlan {
            delete: Vec::new(),
            thin: vec![Thinning {
                segment: name(hour),
                keep: vec![store[0].snapshots[2]],
            }],
        }
    );
}

#[test]
fn a_segment_straddling_a_boundary_waits_until_it_has_crossed() {
    // The hour that began 24 h 30 min ago is half inside the first Tier.
    let straddling = NOW - DAY - 30 * 60 - 30 * 60;
    let store = segments(&[straddling + 60, straddling + 3000, NOW - 60]);

    assert_eq!(
        plan_retention(
            &store,
            at(NOW - 30 * 60),
            &Ladder::default(),
            Newest::Survives
        ),
        RetentionPlan::default(),
        "half of it is younger than 24 h"
    );
    assert_eq!(
        plan_retention(&store, at(NOW), &Ladder::default(), Newest::Survives)
            .thin
            .len(),
        1,
        "once wholly past the boundary, it is thinned"
    );
}

#[test]
fn a_tier_coarser_than_the_segment_deletes_whole_segments() {
    // Three hours of one day, wholly in the daily Tier: only the last of the day survives.
    let day = NOW - 10 * DAY - 14 * HOUR;
    let store = segments(&[
        day + 3 * HOUR + 60,
        day + 9 * HOUR + 60,
        day + 9 * HOUR + 600,
        day + 20 * HOUR + 60,
        NOW - 60,
    ]);

    let plan = plan_retention(&store, at(NOW), &Ladder::default(), Newest::Survives);

    assert_eq!(
        plan,
        RetentionPlan {
            delete: vec![name(day + 3 * HOUR), name(day + 9 * HOUR)],
            thin: Vec::new(),
        },
        "the hour holding the day's last Snapshot is kept whole: it holds only that one"
    );
}

#[test]
fn past_the_last_tier_everything_goes_except_the_newest_snapshot() {
    let old = NOW - 40 * DAY;
    let store = segments(&[old + 60, old + 120, old + HOUR + 60, old + 2 * HOUR + 60]);

    let plan = plan_retention(&store, at(NOW), &Ladder::default(), Newest::Survives);

    assert_eq!(
        plan,
        RetentionPlan {
            delete: vec![name(old), name(old + HOUR)],
            thin: Vec::new(),
        },
        "the Root's newest Snapshot is the record of its present state, however old"
    );
}

#[test]
fn a_second_pass_over_the_result_changes_nothing() {
    let mut moments = Vec::new();
    // A Snapshot every twenty minutes for forty days.
    let mut moment = NOW - 40 * DAY;
    while moment < NOW {
        moments.push(moment);
        moment += 20 * 60;
    }
    let store = segments(&moments);
    let ladder = Ladder::default();

    let first = plan_retention(&store, at(NOW), &ladder, Newest::Survives);
    let after = applied(store, &first);
    assert_ne!(first, RetentionPlan::default());

    assert_eq!(
        plan_retention(&after, at(NOW), &ladder, Newest::Survives),
        RetentionPlan::default(),
        "thinning happens at most once per Segment"
    );
    let hourly_tier = after
        .iter()
        .filter(|segment| segment.name.span().0 < at(NOW - DAY - HOUR))
        .filter(|segment| segment.name.span().0 > at(NOW - 7 * DAY));
    assert!(
        hourly_tier
            .clone()
            .all(|segment| segment.snapshots.len() == 1)
    );
    assert_eq!(
        after
            .iter()
            .filter(|s| s.name.span().0 < at(NOW - 30 * DAY))
            .count(),
        0
    );
}

#[test]
fn a_ladder_must_grow_older_and_coarser() {
    let tier = |age: u64, keep: Keep| Tier {
        age: Duration::from_secs(age),
        keep,
    };
    let every = |seconds: u64| Keep::Every(Duration::from_secs(seconds));

    assert_eq!(Ladder::new(Vec::new()), Err(InvalidLadder::Empty));
    assert_eq!(
        Ladder::new(vec![tier(DAY, every(HOUR))]),
        Err(InvalidLadder::NewestThinned),
        "the most recent history is never thinned"
    );
    assert_eq!(
        Ladder::new(vec![tier(DAY, Keep::All), tier(DAY, every(HOUR))]),
        Err(InvalidLadder::AgesNotIncreasing { tier: 2 })
    );
    assert_eq!(
        Ladder::new(vec![tier(DAY, Keep::All), tier(2 * DAY, every(0))]),
        Err(InvalidLadder::NothingKept { tier: 2 })
    );
    assert_eq!(
        Ladder::new(vec![
            tier(DAY, Keep::All),
            tier(2 * DAY, every(HOUR)),
            tier(7 * DAY, Keep::All),
        ]),
        Err(InvalidLadder::Finer { tier: 3 })
    );
    assert_eq!(
        Ladder::new(vec![
            tier(DAY, Keep::All),
            tier(2 * DAY, every(2 * HOUR)),
            tier(7 * DAY, every(3 * HOUR)),
        ]),
        Err(InvalidLadder::Finer { tier: 3 }),
        "each period must hold whole periods of the one before, or a survivor could be thinned away"
    );
    assert!(
        Ladder::new(vec![
            tier(DAY, Keep::All),
            tier(2 * DAY, every(HOUR)),
            tier(7 * DAY, every(6 * HOUR)),
        ])
        .is_ok()
    );
}

#[test]
fn what_a_tier_keeps_is_written_by_name_or_by_period() {
    let hour = Duration::from_secs(HOUR);
    for (written, keep, name) in [
        ("all", Keep::All, "all"),
        ("hourly", Keep::Every(hour), "1h"),
        ("daily", Keep::Every(24 * hour), "1d"),
        ("weekly", Keep::Every(7 * 24 * hour), "7d"),
        ("6h", Keep::Every(6 * hour), "6h"),
    ] {
        assert_eq!(Keep::parse(written), Some(keep), "{written}");
        assert_eq!(keep.name(), name);
    }
    assert_eq!(Keep::parse("fortnightly"), None);
}

#[test]
fn a_root_no_longer_watched_lets_even_its_newest_snapshot_age() {
    let old = NOW - 40 * DAY;
    let store = segments(&[old + 60, old + HOUR + 60]);

    let plan = plan_retention(&store, at(NOW), &Ladder::default(), Newest::Ages);

    assert_eq!(
        plan,
        RetentionPlan {
            delete: vec![name(old), name(old + HOUR)],
            thin: Vec::new(),
        },
        "its last state is its Pin's to hold, not the ladder's (ticket 53)"
    );
}
