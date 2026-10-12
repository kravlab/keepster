//! What a Snapshot's identity promises the timeline: an order equal to the order of capture, and a
//! moment that a time filter can compare against without a second ordering.

use std::time::{Duration, SystemTime};

use keepster_core::{ChangeKind, SnapshotId, Trigger};

fn at_millis(epoch_millis: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(epoch_millis)
}

/// 2026-09-30T14:37:05Z, in milliseconds.
const MOMENT: u64 = 1_790_779_025_000;

#[test]
fn a_snapshot_in_the_same_millisecond_still_sorts_after_the_one_before() {
    // Two captures can land in one millisecond, and a ULID's random half would then order them by
    // chance — showing the later Snapshot first about half the time. Many draws make chance visible.
    let mut previous = SnapshotId::following(None, at_millis(MOMENT));

    for _ in 0..1_000 {
        let next = SnapshotId::following(Some(previous), at_millis(MOMENT));

        assert!(next > previous, "{next} must sort after {previous}");
        assert_eq!(next.moment(), at_millis(MOMENT), "and keep its moment");
        previous = next;
    }
}

#[test]
fn a_later_snapshot_keeps_its_own_moment() {
    let first = SnapshotId::following(None, at_millis(MOMENT));
    let later = SnapshotId::following(Some(first), at_millis(MOMENT + 60_000));

    assert_eq!(later.moment(), at_millis(MOMENT + 60_000));
    assert!(later > first);
}

#[test]
fn a_clock_that_went_backwards_is_not_hidden() {
    // Inventing a moment to keep the order would record a time at which nothing happened; the index
    // notices the disorder instead.
    let first = SnapshotId::following(None, at_millis(MOMENT));
    let earlier = SnapshotId::following(Some(first), at_millis(MOMENT - 60_000));

    assert_eq!(earlier.moment(), at_millis(MOMENT - 60_000));
}

#[test]
fn the_moment_is_the_millisecond_the_identity_was_minted_in() {
    let id = SnapshotId::generate(at_millis(MOMENT) + Duration::from_micros(700));

    assert_eq!(
        id.moment(),
        at_millis(MOMENT),
        "sub-millisecond time is not carried"
    );
}

#[test]
fn first_after_separates_at_or_before_from_after() {
    let bound = SnapshotId::first_after(at_millis(MOMENT));

    for _ in 0..100 {
        let at = SnapshotId::generate(at_millis(MOMENT));
        let next = SnapshotId::generate(at_millis(MOMENT + 1));

        assert!(at < bound, "taken at the moment is at or before it");
        assert!(next >= bound, "taken a millisecond later is after it");
    }
}

#[test]
fn change_kinds_and_triggers_round_trip_through_their_names() {
    for kind in [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted] {
        assert_eq!(ChangeKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(
        Trigger::parse(Trigger::Manual.as_str()),
        Some(Trigger::Manual)
    );
    assert_eq!(ChangeKind::parse("renamed"), None);
}
