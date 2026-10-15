//! Search across a Root's Snapshots (ticket 59, ticket 34): a path by its name, from the change rows the
//! index already holds, and content by a walk over the distinct versions git stores — each read once,
//! however many Snapshots held it, and attributed to the range of Snapshots it was current for.
//!
//! No full-text index: rebuilding one would re-read every version in the store, which would make the
//! index's promise — deleted and rebuilt from git at any time — false rather than slow.

use std::collections::{BTreeMap, HashMap};
use std::ops::ControlFlow;

use keepster_core::{
    ChangeKind, ContentQuery, LineMatch, PathQuery, RootPath, SnapshotId, WithheldReason, is_binary,
};

use super::{HistoryStore, StoreError, withheld_at};

/// A path the history holds, as a path search finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathFound {
    /// The path, Root-relative.
    pub path: RootPath,
    /// How many Snapshots changed it.
    pub snapshots: u64,
    /// The first of them.
    pub first: SnapshotId,
    /// The last of them.
    pub last: SnapshotId,
    /// Whether the newest Snapshot still has it: its last change did not delete it.
    pub exists: bool,
}

/// One path's content between two Snapshots: what it held from `first` to `last`, both included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentVersion {
    /// The path, Root-relative.
    pub path: RootPath,
    /// The first Snapshot it was current in.
    pub first: SnapshotId,
    /// The last Snapshot it was current in.
    pub last: SnapshotId,
}

/// A path met on the way that has no content to search, and why (ADR 0009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithheldMet {
    /// The path, Root-relative.
    pub path: RootPath,
    /// Why it was withheld.
    pub reason: WithheldReason,
    /// The first Snapshot it was withheld in.
    pub snapshot: SnapshotId,
}

/// What a content search of one Root will read, known before the first byte of it is.
#[derive(Debug, Clone, Default)]
pub struct ContentPlan {
    /// Every distinct version, with each place it was current — read once whatever it is held by.
    versions: Vec<(git2::Oid, Vec<ContentVersion>)>,
    /// Their sizes, summed: the cost.
    pub bytes: u64,
    /// Every path met with no content to search.
    pub withheld: Vec<WithheldMet>,
}

impl ContentPlan {
    /// How many distinct versions it reads.
    pub fn versions(&self) -> u64 {
        self.versions.len() as u64
    }
}

/// What a content search found, as it finds it.
#[derive(Debug)]
pub enum ContentFound<'a> {
    /// A line of a version, which `version` held from its first Snapshot to its last.
    Match {
        /// Where, and when.
        version: &'a ContentVersion,
        /// The version's own identity: equal for equal content.
        blob: String,
        /// The line.
        line: &'a LineMatch,
    },
    /// One more version has been looked at.
    Scanned,
}

/// What a content search looked at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchCounts {
    /// Versions searched as text.
    pub scanned: u64,
    /// Versions skipped as binary.
    pub binary: u64,
    /// Versions the store could not read.
    pub unreadable: u64,
}

impl HistoryStore {
    /// The paths the history holds that `query` finds, in path order, at most `limit` of them — and
    /// whether there were more.
    pub fn find_paths(
        &self,
        query: &PathQuery,
        limit: usize,
    ) -> Result<(Vec<PathFound>, bool), StoreError> {
        self.catch_up()?;
        let (histories, more) = self
            .index()?
            .path_histories(&|path| query.matches(path), limit)?;

        Ok((
            histories
                .into_iter()
                .map(|history| PathFound {
                    path: history.path,
                    snapshots: history.changes,
                    first: history.first,
                    last: history.last,
                    exists: history.exists,
                })
                .collect(),
            more,
        ))
    }

    /// What searching the content at or beneath `under` reads: every distinct version, each with the
    /// ranges of Snapshots it was current for, their total size, and every Withheld path met.
    pub fn content_plan(&self, under: &RootPath) -> Result<ContentPlan, StoreError> {
        self.catch_up()?;
        let (changes, snapshots) = {
            let index = self.index()?;
            (index.changes_under(under)?, index.snapshot_ids()?)
        };
        let Some(newest) = snapshots.last().copied() else {
            return Ok(ContentPlan::default());
        };
        let position: HashMap<SnapshotId, usize> = snapshots
            .iter()
            .enumerate()
            .map(|(at, id)| (*id, at))
            .collect();
        // The Snapshot just before `id`: the last one a version replaced in `id` was current in.
        let before = |id: SnapshotId| {
            position
                .get(&id)
                .and_then(|at| at.checked_sub(1))
                .map(|at| snapshots[at])
        };

        let mut by_blob: BTreeMap<git2::Oid, Vec<ContentVersion>> = BTreeMap::new();
        let mut withheld_candidates: Vec<(RootPath, SnapshotId)> = Vec::new();
        let mut current: Option<(RootPath, git2::Oid, SnapshotId)> = None;
        for change in &changes {
            let same_path = current
                .as_ref()
                .is_some_and(|(path, _, _)| *path == change.path);
            // The same content again — its record changed, not it — goes on being one version.
            if same_path && current.as_ref().map(|(_, blob, _)| Some(*blob)) == Some(change.blob) {
                continue;
            }
            if let Some((path, blob, first)) = current.take() {
                let last = if path == change.path {
                    before(change.snapshot)
                } else {
                    Some(newest)
                };
                if let Some(last) = last {
                    by_blob
                        .entry(blob)
                        .or_default()
                        .push(ContentVersion { path, first, last });
                }
            }
            match change.blob {
                Some(blob) => current = Some((change.path.clone(), blob, change.snapshot)),
                None if change.kind != ChangeKind::Deleted => {
                    withheld_candidates.push((change.path.clone(), change.snapshot));
                }
                None => {}
            }
        }
        if let Some((path, blob, first)) = current.take() {
            by_blob.entry(blob).or_default().push(ContentVersion {
                path,
                first,
                last: newest,
            });
        }

        let odb = self.repository.odb().map_err(self.failed("read"))?;
        let bytes = by_blob
            .keys()
            .filter_map(|blob| odb.read_header(*blob).ok())
            .fold(0_u64, |bytes, (size, _)| bytes.saturating_add(size as u64));
        let mut versions: Vec<(git2::Oid, Vec<ContentVersion>)> = by_blob.into_iter().collect();
        versions.sort_by(|a, b| {
            a.1.first()
                .map(|v| &v.path)
                .cmp(&b.1.first().map(|v| &v.path))
        });

        Ok(ContentPlan {
            versions,
            bytes,
            withheld: self.withheld_met(&withheld_candidates)?,
        })
    }

    /// Which of `candidates` — a path named without content, in a Snapshot — are Withheld there, and
    /// why: each path once per reason, at the first Snapshot it was met in.
    fn withheld_met(
        &self,
        candidates: &[(RootPath, SnapshotId)],
    ) -> Result<Vec<WithheldMet>, StoreError> {
        let mut sidecars = HashMap::new();
        let mut met: BTreeMap<(RootPath, WithheldReason), SnapshotId> = BTreeMap::new();
        for (path, snapshot) in candidates {
            if !sidecars.contains_key(snapshot) {
                let tree = self.snapshot_tree(*snapshot)?;
                sidecars.insert(*snapshot, self.sidecar(&tree)?.sidecar);
            }
            if let Some(reason) = sidecars
                .get(snapshot)
                .and_then(|sidecar| withheld_at(sidecar, path))
            {
                met.entry((path.clone(), reason)).or_insert(*snapshot);
            }
        }

        Ok(met
            .into_iter()
            .map(|((path, reason), snapshot)| WithheldMet {
                path,
                reason,
                snapshot,
            })
            .collect())
    }

    /// Search every version `plan` names for `query`, handing each matching line to `visit` — once per
    /// place the version was current, never once per Snapshot — until the walk ends or `visit` says to
    /// stop. A binary version is skipped and counted, never searched.
    pub fn search_content(
        &self,
        plan: &ContentPlan,
        query: &ContentQuery,
        visit: &mut dyn FnMut(ContentFound<'_>) -> ControlFlow<()>,
    ) -> SearchCounts {
        let mut counts = SearchCounts::default();
        for (blob, versions) in &plan.versions {
            match self.repository.find_blob(*blob) {
                // Still looked at, as far as the reader's progress goes.
                Err(_) => counts.unreadable += 1,
                Ok(found) if is_binary(found.content()) => counts.binary += 1,
                Ok(found) => {
                    counts.scanned += 1;
                    let id = blob.to_string();
                    for line in query.lines(found.content()) {
                        for version in versions {
                            let found = ContentFound::Match {
                                version,
                                blob: id.clone(),
                                line: &line,
                            };
                            if visit(found).is_break() {
                                return counts;
                            }
                        }
                    }
                }
            }
            if visit(ContentFound::Scanned).is_break() {
                return counts;
            }
        }

        counts
    }
}
