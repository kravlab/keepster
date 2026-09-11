// PROTOTYPE fixture (ticket 09): dense Rust, to judge highlight quality — lifetimes, generics,
// attributes, macros, raw strings, doc comments — and a realistic refactor-shaped diff.
export const RUST_BEFORE = `use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use git2::{Repository, Signature};

use crate::model::{Snapshot, SnapshotId, Trigger};

/// Writes a Snapshot into a Root's History Store.
///
/// The index is ours, never the watched directory's: see ADR 0001.
#[derive(Debug)]
pub struct SnapshotWriter<'repo> {
    repo: &'repo Repository,
    root_id: String,
    segment_ref: String,
}

impl<'repo> SnapshotWriter<'repo> {
    pub fn new(repo: &'repo Repository, root_id: impl Into<String>) -> Self {
        let root_id = root_id.into();
        let segment_ref = format!("refs/keepster/segments/{}/current", root_id);
        Self { repo, root_id, segment_ref }
    }

    pub fn write(&mut self, paths: &[PathBuf], trigger: Trigger) -> Result<Snapshot> {
        let mut index = self.repo.index().context("opening the store index")?;
        index.clear()?;

        for path in paths {
            index
                .add_path(path)
                .with_context(|| format!("staging {}", path.display()))?;
        }

        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = Signature::now("keepster", "keepster@localhost")?;
        let parent = self.tip()?;
        let parents: Vec<_> = parent.iter().collect();

        let commit = self.repo.commit(
            Some(&self.segment_ref),
            &sig,
            &sig,
            &message(trigger),
            &tree,
            &parents,
        )?;

        Ok(Snapshot {
            id: SnapshotId::new(),
            commit: commit.to_string(),
            taken_at: SystemTime::now(),
            trigger,
            incomplete: false,
        })
    }

    fn tip(&self) -> Result<Option<git2::Commit<'repo>>> {
        match self.repo.find_reference(&self.segment_ref) {
            Ok(r) => Ok(Some(r.peel_to_commit()?)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn message(trigger: Trigger) -> String {
    format!(r#"snapshot

Keepster-Trigger: {trigger:?}
"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_snapshot_is_parentless() {
        // a Segment always starts from a parentless commit
        assert!(true);
    }
}
`

export const RUST_AFTER = `use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use git2::{Repository, Signature};

use crate::model::{Skipped, Snapshot, SnapshotId, Trigger};

/// Writes a Snapshot into a Root's History Store.
///
/// The index is ours, never the watched directory's: see ADR 0001.
/// A file that cannot be read does not fail the Snapshot; it is recorded as skipped and the
/// Snapshot is marked incomplete (ticket 05).
#[derive(Debug)]
pub struct SnapshotWriter<'repo> {
    repo: &'repo Repository,
    root_id: String,
    segment_ref: String,
}

impl<'repo> SnapshotWriter<'repo> {
    pub fn new(repo: &'repo Repository, root_id: impl Into<String>) -> Self {
        let root_id = root_id.into();
        let segment_ref = format!("refs/keepster/segments/{}/current", root_id);
        Self { repo, root_id, segment_ref }
    }

    pub fn write(&mut self, paths: &[PathBuf], trigger: Trigger) -> Result<Snapshot> {
        let mut index = self.repo.index().context("opening the store index")?;
        index.clear()?;

        let mut skipped: Vec<Skipped> = Vec::new();
        for path in paths {
            if let Err(err) = index.add_path(path) {
                skipped.push(Skipped::new(path, err.raw_code()));
                continue;
            }
        }

        let tree_oid = index.write_tree()?;
        let tree = self.repo.find_tree(tree_oid)?;
        let sig = Signature::now("keepster", "keepster@localhost")?;
        let parent = self.tip()?;
        let parents: Vec<_> = parent.iter().collect();

        let commit = self.repo.commit(
            Some(&self.segment_ref),
            &sig,
            &sig,
            &message(trigger, &skipped),
            &tree,
            &parents,
        )?;

        Ok(Snapshot {
            id: SnapshotId::new(),
            commit: commit.to_string(),
            taken_at: SystemTime::now(),
            trigger,
            incomplete: !skipped.is_empty(),
        })
    }

    fn tip(&self) -> Result<Option<git2::Commit<'repo>>> {
        match self.repo.find_reference(&self.segment_ref) {
            Ok(r) => Ok(Some(r.peel_to_commit()?)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

fn message(trigger: Trigger, skipped: &[Skipped]) -> String {
    let mut msg = format!(r#"snapshot

Keepster-Trigger: {trigger:?}
"#);
    for s in skipped {
        msg.push_str(&format!("Keepster-Skipped: {} errno={}\\n", s.path.display(), s.errno));
    }
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_snapshot_is_parentless() {
        // a Segment always starts from a parentless commit
        assert!(true);
    }

    #[test]
    fn unreadable_file_marks_the_snapshot_incomplete() {
        assert!(true);
    }
}
`
