//! Which Snapshots survive (ticket 51, ticket 06): the ladder, and the plan a Retention pass carries out.
//!
//! A ladder is a list of Tiers by age. Within a Tier a Snapshot survives when it is the last of its
//! period — the state the next period started from — and past the last Tier nothing does, except the
//! Root's newest Snapshot, which is the record of its present state however old it is. The plan acts
//! only on a Segment lying wholly inside one Tier, never on one a boundary still runs through: that is
//! what thins each Segment at most once per Tier and makes a second pass change nothing.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::segment::SegmentName;
use crate::snapshot::SnapshotId;

/// What one Tier keeps of the Snapshots it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    /// Every one.
    All,
    /// The last of each period this long, periods counted from the epoch in UTC — so an hour and a day
    /// are the clock's and the calendar's.
    Every(Duration),
}

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);

impl Keep {
    /// What a Tier's `keep` names: `all`, `hourly`, `daily` or `weekly` — one hour, one day, seven days
    /// — or a period such as `6h`.
    pub fn parse(written: &str) -> Option<Self> {
        match written {
            "all" => Some(Self::All),
            "hourly" => Some(Self::Every(HOUR)),
            "daily" => Some(Self::Every(DAY)),
            "weekly" => Some(Self::Every(7 * DAY)),
            period => crate::parse_duration(period).ok().map(Self::Every),
        }
    }

    /// How it is written back: `all`, or the period as [`crate::format_duration`] spells it.
    pub fn name(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Every(period) => crate::format_duration(*period),
        }
    }
}

/// One step of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tier {
    /// Snapshots younger than this, and no younger than the Tier before reaches, are this Tier's.
    pub age: Duration,
    /// What survives of them.
    pub keep: Keep,
}

/// Why a list of Tiers is not a ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidLadder {
    /// There is no Tier at all, which would discard everything but the newest Snapshot at once.
    #[error("a ladder needs at least one Tier")]
    Empty,
    /// The youngest Tier thins: the most recent history is the window this product exists to serve,
    /// so it is bounded by age alone and kept whole (ticket 06).
    #[error("the first Tier must keep `all`")]
    NewestThinned,
    /// A Tier, counted from 1, is no older than the one before it.
    #[error("Tier {tier} must be older than the Tier before it")]
    AgesNotIncreasing {
        /// Which, from 1.
        tier: usize,
    },
    /// A Tier keeps one Snapshot per period of no length.
    #[error("Tier {tier} keeps one Snapshot per period of no length")]
    NothingKept {
        /// Which, from 1.
        tier: usize,
    },
    /// A Tier keeps more than the one before it, or periods that do not each hold whole periods of the
    /// one before — so a Snapshot one Tier kept as the last of its period could be thinned away by the
    /// next, and that period lose every Snapshot.
    #[error("Tier {tier} must keep whole periods of the Tier before it, or keep less")]
    Finer {
        /// Which, from 1.
        tier: usize,
    },
}

/// The Tiers, from the youngest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ladder {
    tiers: Vec<Tier>,
}

impl Ladder {
    /// A ladder of `tiers`, from the youngest: the first keeping everything, each older than the last,
    /// each keeping whole periods of the one before it.
    pub fn new(tiers: Vec<Tier>) -> Result<Self, InvalidLadder> {
        match tiers.first() {
            None => return Err(InvalidLadder::Empty),
            Some(first) if first.keep != Keep::All => return Err(InvalidLadder::NewestThinned),
            Some(_) => {}
        }
        for (position, tier) in tiers.iter().enumerate() {
            let number = position + 1;
            if tier.keep == Keep::Every(Duration::ZERO) {
                return Err(InvalidLadder::NothingKept { tier: number });
            }
            let Some(before) = position.checked_sub(1).map(|before| tiers[before]) else {
                continue;
            };
            if tier.age <= before.age {
                return Err(InvalidLadder::AgesNotIncreasing { tier: number });
            }
            let whole = match (before.keep, tier.keep) {
                (Keep::All, _) => true,
                (Keep::Every(_), Keep::All) => false,
                (Keep::Every(finer), Keep::Every(coarser)) => {
                    coarser.as_millis().is_multiple_of(finer.as_millis())
                }
            };
            if !whole {
                return Err(InvalidLadder::Finer { tier: number });
            }
        }

        Ok(Self { tiers })
    }

    /// The Tiers, from the youngest.
    pub fn tiers(&self) -> &[Tier] {
        &self.tiers
    }

    /// The Tier whose ages hold every one of `oldest` down to `youngest`, as its position — or
    /// `Some(len)` when even the youngest is past the last Tier, and `None` when a boundary runs
    /// between them.
    fn holding(&self, youngest: Duration, oldest: Duration) -> Option<usize> {
        let mut floor = Duration::ZERO;
        for (position, tier) in self.tiers.iter().enumerate() {
            if youngest >= floor && oldest < tier.age {
                return Some(position);
            }
            floor = tier.age;
        }

        (youngest >= floor).then_some(self.tiers.len())
    }
}

impl Default for Ladder {
    /// Everything for a day, one an hour to a week, one a day to a month (ticket 06).
    fn default() -> Self {
        Self {
            tiers: vec![
                Tier {
                    age: DAY,
                    keep: Keep::All,
                },
                Tier {
                    age: 7 * DAY,
                    keep: Keep::Every(HOUR),
                },
                Tier {
                    age: 30 * DAY,
                    keep: Keep::Every(DAY),
                },
            ],
        }
    }
}

/// One Segment and the Snapshots it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentContents {
    /// Which, and so the moments it covers.
    pub name: SegmentName,
    /// Its Snapshots, oldest first.
    pub snapshots: Vec<SnapshotId>,
}

/// A Segment to rewrite with fewer Snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thinning {
    /// Which.
    pub segment: SegmentName,
    /// The Snapshots it keeps, oldest first, each keeping its identity.
    pub keep: Vec<SnapshotId>,
}

/// What a Retention pass does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionPlan {
    /// Segments none of whose Snapshots survives, in name order: each a ref deletion.
    pub delete: Vec<SegmentName>,
    /// Segments some of whose Snapshots survive, in name order: each re-chained.
    pub thin: Vec<Thinning>,
}

impl RetentionPlan {
    /// Whether the pass has nothing to do.
    pub fn is_empty(&self) -> bool {
        self.delete.is_empty() && self.thin.is_empty()
    }
}

/// What a Retention pass at `now` does to `segments` under `ladder`.
///
/// A period's survivor is the last Snapshot in it across every Segment, so a Tier coarser than the
/// Segments deletes all but the one holding it, and a Tier as fine as them thins within each. A Segment
/// that reaches a coarser Tier without having been thinned by one as fine as itself — a daemon that was
/// down for the whole of the finer Tier — is thinned there too.
pub fn plan_retention(
    segments: &[SegmentContents],
    now: SystemTime,
    ladder: &Ladder,
) -> RetentionPlan {
    let newest = segments
        .iter()
        .flat_map(|segment| segment.snapshots.iter())
        .max()
        .copied();
    // The last Snapshot of every period, for each period length the ladder keeps by.
    let mut last: HashMap<(u128, u128), SnapshotId> = HashMap::new();
    for tier in ladder.tiers() {
        let Keep::Every(period) = tier.keep else {
            continue;
        };
        for snapshot in segments.iter().flat_map(|segment| &segment.snapshots) {
            let key = (period.as_millis(), bucket(*snapshot, period));
            let kept = last.entry(key).or_insert(*snapshot);
            *kept = (*kept).max(*snapshot);
        }
    }

    let mut ordered: Vec<&SegmentContents> = segments.iter().collect();
    ordered.sort_by(|a, b| a.name.cmp(&b.name));

    let mut plan = RetentionPlan::default();
    for segment in ordered {
        let (start, end) = segment.name.span();
        // Not over yet, or not wholly inside one Tier: left as it is until it has crossed.
        let (Ok(youngest), Ok(oldest)) = (now.duration_since(end), now.duration_since(start))
        else {
            continue;
        };
        let Some(position) = ladder.holding(youngest, oldest) else {
            continue;
        };
        let survives = |snapshot: &SnapshotId| {
            Some(*snapshot) == newest
                || match ladder.tiers().get(position).map(|tier| tier.keep) {
                    Some(Keep::All) => true,
                    Some(Keep::Every(period)) => {
                        last.get(&(period.as_millis(), bucket(*snapshot, period))) == Some(snapshot)
                    }
                    // Past the last Tier.
                    None => false,
                }
        };

        let mut keep: Vec<SnapshotId> =
            segment.snapshots.iter().copied().filter(survives).collect();
        keep.sort();
        if keep.is_empty() {
            plan.delete.push(segment.name.clone());
        } else if keep.len() < segment.snapshots.len() {
            plan.thin.push(Thinning {
                segment: segment.name.clone(),
                keep,
            });
        }
    }

    plan
}

/// Which period of `length` the Snapshot was taken in, counted from the epoch.
fn bucket(snapshot: SnapshotId, length: Duration) -> u128 {
    let taken = snapshot
        .moment()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());

    taken / length.as_millis().max(1)
}
