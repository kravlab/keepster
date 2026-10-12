//! What a History Store holds, described for a diagnostics bundle (ticket 63, ADR 0013).
//!
//! Read from refs, commit messages and the derived index alone: **never a blob**, because no bytes of a
//! file from history may enter the bundle under any flag, and not reading them is the only guarantee
//! that needs no pattern matching. Nor is the index brought up to date first — a report that repaired
//! what it reports on would hide the lag it exists to show.

use std::fs;
use std::io;
use std::path::Path;

use keepster_core::SnapshotId;

use super::{HistoryStore, PIN_REFS, SEGMENT_REFS, StoreError};
use crate::{format, index};

/// The shape of one store: its Segments, its Pins, how far its index got, and the space it takes.
#[derive(Debug)]
pub struct Topology {
    /// Every ref under the Segments, in name order — which is chronological.
    pub segments: Vec<SegmentShape>,
    /// Every Pin, in name order.
    pub pins: Vec<PinShape>,
    /// Every source the index tracks or should track, in source order, or why the index could not be
    /// read as it is.
    pub watermarks: Result<Vec<Watermark>, StoreError>,
    /// The bytes the repository takes on disk.
    pub history_bytes: u64,
    /// The bytes the derived index takes on disk, its write-ahead files included; zero when there is
    /// none.
    pub index_bytes: u64,
}

/// One Segment's ref and the chain it ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentShape {
    /// The ref's last component: a period, perhaps followed by a sibling's format (ADR 0010) — or
    /// anything else, for a ref this product did not write, which is reported all the same.
    pub name: String,
    /// The commit at its tip.
    pub tip: String,
    /// The content format its Snapshots are in, or `None` when it names none this build knows of.
    pub format: Option<u32>,
    /// How many of its commits hold a Snapshot.
    pub snapshots: usize,
    /// How many of its commits hold none: written by something other than this product.
    pub foreign: usize,
    /// Its oldest Snapshot.
    pub oldest: Option<SnapshotId>,
    /// Its newest Snapshot.
    pub newest: Option<SnapshotId>,
}

/// One Pin's ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinShape {
    /// The ref's last component, which is the pinned Snapshot's identity (ADR 0002).
    pub snapshot: String,
    /// The commit it names.
    pub tip: String,
    /// The content format of the Snapshot it holds, or `None` when it names none this build knows of.
    pub format: Option<u32>,
}

/// How far the index got with one source: a Segment, or a Pin as `pins/` and its identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watermark {
    /// The source, as the index names it.
    pub source: String,
    /// The commit at the source's tip now, or `None` for a source the index still names after its ref
    /// went.
    pub tip: Option<String>,
    /// The commit the index caught up to, or `None` for a source it has not indexed yet. Equal to `tip`
    /// once the index is up to date.
    pub indexed: Option<String>,
}

impl HistoryStore {
    /// This store's shape, read without changing anything.
    pub fn topology(&self) -> Result<Topology, StoreError> {
        let mut segments = Vec::new();
        for (name, tip) in self.leaves(SEGMENT_REFS)? {
            let mut snapshots = Vec::new();
            let mut foreign = 0;
            for commit in self.chain(tip)? {
                match self.snapshot_of(commit)? {
                    Some(snapshot) => snapshots.push(snapshot),
                    None => foreign += 1,
                }
            }
            snapshots.sort();
            segments.push(SegmentShape {
                name,
                tip: tip.to_string(),
                format: self.format_number(tip)?,
                snapshots: snapshots.len(),
                foreign,
                oldest: snapshots.first().copied(),
                newest: snapshots.last().copied(),
            });
        }

        let mut pins = Vec::new();
        for (snapshot, tip) in self.leaves(PIN_REFS)? {
            pins.push(PinShape {
                snapshot,
                tip: tip.to_string(),
                format: self.format_number(tip)?,
            });
        }

        Ok(Topology {
            segments,
            pins,
            watermarks: self.watermarks(),
            history_bytes: disk_usage(&self.path).map_err(|source| StoreError::Measure {
                path: self.path.clone(),
                source,
            })?,
            index_bytes: index_files(&self.index_path)
                .iter()
                .filter_map(|file| fs::symlink_metadata(file).ok())
                .map(|metadata| metadata.len())
                .sum(),
        })
    }

    /// Every ref directly under `refs`, by its last component and the commit it names, in name order.
    /// A ref that names no commit, or whose name is not UTF-8, was not written by this product and
    /// describes nothing here.
    fn leaves(&self, refs: &str) -> Result<Vec<(String, git2::Oid)>, StoreError> {
        let mut leaves = Vec::new();
        for reference in self
            .repository
            .references_glob(&format!("{refs}/*"))
            .map_err(self.failed("read"))?
        {
            let reference = reference.map_err(self.failed("read"))?;
            let Some(name) = reference
                .name()
                .ok()
                .and_then(|name| name.rsplit('/').next())
            else {
                continue;
            };
            let Ok(commit) = reference.peel_to_commit() else {
                continue;
            };
            leaves.push((name.to_owned(), commit.id()));
        }
        leaves.sort();

        Ok(leaves)
    }

    /// The content format of the Segment or Pin at `tip`, as a number.
    fn format_number(&self, tip: git2::Oid) -> Result<Option<u32>, StoreError> {
        Ok(format::of(&self.repository, tip)
            .map_err(self.failed("read"))?
            .map(keepster_core::ContentFormat::number))
    }

    /// Every source's tip beside the index's watermark for it, joined by source.
    fn watermarks(&self) -> Result<Vec<Watermark>, StoreError> {
        let tips = index::source_tips(&self.repository).map_err(self.failed("read"))?;
        let marks = index::watermarks_at(&self.index_path)?.unwrap_or_default();
        let mut sources: Vec<&String> = tips.keys().chain(marks.keys()).collect();
        sources.sort();
        sources.dedup();

        Ok(sources
            .into_iter()
            .map(|source| Watermark {
                source: source.clone(),
                tip: tips.get(source).map(ToString::to_string),
                indexed: marks.get(source).map(ToString::to_string),
            })
            .collect())
    }
}

/// The index file and the write-ahead files SQLite keeps beside it.
fn index_files(index: &Path) -> [std::path::PathBuf; 3] {
    ["", "-wal", "-shm"].map(|suffix| {
        let mut name = index.as_os_str().to_owned();
        name.push(suffix);
        name.into()
    })
}

/// The bytes every file under `path` takes, links counted as themselves and never followed.
fn disk_usage(path: &Path) -> io::Result<u64> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Ok(metadata.len());
    }

    let mut total = 0;
    for entry in fs::read_dir(path)? {
        total += disk_usage(&entry?.path())?;
    }

    Ok(total)
}
