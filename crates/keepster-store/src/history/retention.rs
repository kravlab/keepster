//! Retention's half that touches the store (ticket 51): what the Segments hold, and the plan carried
//! out — a Segment none of whose Snapshots survives is a ref deletion, one some of whose do is re-chained
//! with only those, each keeping its identity. What survives is decided in `keepster_core::retention`.

use std::collections::{BTreeSet, HashMap};
use std::time::SystemTime;

use keepster_core::{
    Ladder, Newest, SegmentContents, SegmentName, SnapshotId, Thinning, plan_retention,
};

use super::{HistoryStore, SEGMENT_REFS, StoreError};
use crate::index;

/// What a Retention pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Retained {
    /// Segments deleted whole.
    pub deleted_segments: u64,
    /// Segments re-chained with fewer Snapshots.
    pub thinned_segments: u64,
    /// Snapshots no longer in any Segment.
    pub removed_snapshots: u64,
    /// Why the derived index lags the Segments, if bringing it up to date failed; the refs changed
    /// regardless, and the next read catches it up.
    pub index_lag: Option<String>,
}

/// One Segment as a pass read it: what it holds, and the tip it held it at.
pub(super) struct Read {
    pub(super) contents: SegmentContents,
    pub(super) tip: git2::Oid,
}

impl HistoryStore {
    /// Every Segment Retention may act on, with its Snapshots oldest first.
    ///
    /// A ref under the Segments whose name is no calendar period, or whose chain holds a commit without
    /// a Snapshot's trailers, was not written by this product, and Retention leaves it as it is — as it
    /// does a Segment in a content format newer than this build (ADR 0010).
    pub fn segments(&self) -> Result<Vec<SegmentContents>, StoreError> {
        Ok(self
            .read_segments()?
            .into_iter()
            .map(|read| read.contents)
            .collect())
    }

    /// One Retention pass at `now` under `ladder`: plan what survives from the Segments as they are,
    /// delete each Segment it condemns and re-chain each it thins, then bring the derived index up to
    /// date — re-reading only what changed.
    ///
    /// A ref is moved only from the tip the pass read, so a Segment written to meanwhile is left as it
    /// is, for the next pass. The daemon keeps captures out of the pass's way with the store's lock all
    /// the same (ticket 06): the check is that a caller without it loses nothing.
    pub fn retain(
        &self,
        ladder: &Ladder,
        now: SystemTime,
        newest: Newest,
    ) -> Result<Retained, StoreError> {
        let read = self.read_segments()?;
        let contents: Vec<SegmentContents> =
            read.iter().map(|read| read.contents.clone()).collect();
        let plan = plan_retention(&contents, now, ladder, newest);
        let tips: HashMap<&SegmentName, git2::Oid> = read
            .iter()
            .map(|read| (&read.contents.name, read.tip))
            .collect();
        let held: HashMap<&SegmentName, usize> = read
            .iter()
            .map(|read| (&read.contents.name, read.contents.snapshots.len()))
            .collect();

        let mut retained = Retained::default();
        for segment in &plan.delete {
            let Some(tip) = tips.get(segment) else {
                continue;
            };
            if self.delete_segment(segment, *tip)? {
                retained.deleted_segments += 1;
                retained.removed_snapshots += count(held.get(segment).copied().unwrap_or(0));
            }
        }
        for thinning in &plan.thin {
            let Some(tip) = tips.get(&thinning.segment) else {
                continue;
            };
            if self.thin(thinning, *tip)? {
                retained.thinned_segments += 1;
                let before = held.get(&thinning.segment).copied().unwrap_or(0);
                retained.removed_snapshots += count(before.saturating_sub(thinning.keep.len()));
            }
        }

        if retained.deleted_segments + retained.thinned_segments > 0 {
            retained.index_lag = self.catch_up().err().map(|error| error.to_string());
        }

        Ok(retained)
    }

    /// Every Segment Retention may act on, as read now.
    pub(super) fn read_segments(&self) -> Result<Vec<Read>, StoreError> {
        let references = self
            .repository
            .references_glob(&format!("{SEGMENT_REFS}/*"))
            .map_err(self.failed("read"))?;

        let mut segments = Vec::new();
        for reference in references {
            let reference = reference.map_err(self.failed("read"))?;
            let Some(name) = reference
                .name()
                .ok()
                .and_then(|name| name.rsplit('/').next())
                .and_then(SegmentName::parse)
            else {
                continue;
            };
            let tip = reference
                .peel_to_commit()
                .map_err(self.failed("read"))?
                .id();
            // A Segment in a format newer than this build is not its to judge, let alone rewrite
            // (ADR 0010): the build that wrote it retires it.
            if !self.is_readable(tip)? {
                continue;
            }
            let mut snapshots = Vec::new();
            let mut foreign = false;
            for commit in self.chain(tip)? {
                match self.snapshot_of(commit)? {
                    Some(snapshot) => snapshots.push(snapshot),
                    None => foreign = true,
                }
            }
            if foreign {
                continue;
            }
            snapshots.sort();
            segments.push(Read {
                contents: SegmentContents { name, snapshots },
                tip,
            });
        }

        Ok(segments)
    }

    /// Delete the Segment `segment` if its tip is still `tip`, saying whether it was deleted.
    fn delete_segment(&self, segment: &SegmentName, tip: git2::Oid) -> Result<bool, StoreError> {
        let mut reference = match self.repository.find_reference(&segment_ref(segment)) {
            Ok(reference) => reference,
            Err(error) if error.code() == git2::ErrorCode::NotFound => return Ok(false),
            Err(source) => return Err(self.failed("read")(source)),
        };
        if reference.target() != Some(tip) {
            return Ok(false);
        }
        reference.delete().map_err(self.failed("written"))?;

        Ok(true)
    }

    /// Re-chain the Segment `thinning` names, read at `tip`, with only the Snapshots it keeps — oldest
    /// first and the first of them parentless, each with its own tree, message and moment, so its
    /// identity, which lives in the message, is unchanged (ADR 0001). Says whether the ref moved: not if
    /// it moved first.
    fn thin(&self, thinning: &Thinning, tip: git2::Oid) -> Result<bool, StoreError> {
        let keep: BTreeSet<SnapshotId> = thinning.keep.iter().copied().collect();

        let mut parent: Option<git2::Commit<'_>> = None;
        for commit in self.chain(tip)? {
            let commit = self
                .repository
                .find_commit(commit)
                .map_err(self.failed("read"))?;
            let id = commit
                .message()
                .ok()
                .and_then(index::trailers)
                .map(|t| t.id);
            if !id.is_some_and(|id| keep.contains(&id)) {
                continue;
            }
            let tree = commit.tree().map_err(self.failed("read"))?;
            let parents: Vec<&git2::Commit<'_>> = parent.iter().collect();
            let rewritten = self
                .repository
                .commit(
                    None,
                    &commit.author(),
                    &commit.committer(),
                    commit.message_raw().unwrap_or_default(),
                    &tree,
                    &parents,
                )
                .map_err(self.failed("written"))?;
            parent = Some(
                self.repository
                    .find_commit(rewritten)
                    .map_err(self.failed("read"))?,
            );
        }

        // A plan never thins a Segment to nothing; one that did would be a deletion.
        let Some(new_tip) = parent else {
            return self.delete_segment(&thinning.segment, tip);
        };
        match self.repository.reference_matching(
            &segment_ref(&thinning.segment),
            new_tip.id(),
            true,
            tip,
            "keepster: thinned by Retention",
        ) {
            Ok(_) => Ok(true),
            // Written to since the pass read it: left for the next pass.
            Err(error) if error.code() == git2::ErrorCode::Modified => Ok(false),
            Err(source) => Err(self.failed("written")(source)),
        }
    }

    /// The identity of the Snapshot `commit` holds, or `None` for a commit this product did not write.
    fn snapshot_of(&self, commit: git2::Oid) -> Result<Option<SnapshotId>, StoreError> {
        let commit = self
            .repository
            .find_commit(commit)
            .map_err(self.failed("read"))?;

        Ok(commit
            .message()
            .ok()
            .and_then(index::trailers)
            .map(|trailers| trailers.id))
    }

    /// The commits of the chain ending at `tip`, oldest first.
    pub(super) fn chain(&self, tip: git2::Oid) -> Result<Vec<git2::Oid>, StoreError> {
        let mut walk = self.repository.revwalk().map_err(self.failed("read"))?;
        walk.push(tip).map_err(self.failed("read"))?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::REVERSE)
            .map_err(self.failed("read"))?;

        walk.collect::<Result<Vec<_>, _>>()
            .map_err(self.failed("read"))
    }
}

/// The ref of the Segment `segment`.
pub(super) fn segment_ref(segment: &SegmentName) -> String {
    format!("{SEGMENT_REFS}/{segment}")
}

fn count(snapshots: usize) -> u64 {
    u64::try_from(snapshots).unwrap_or(u64::MAX)
}
