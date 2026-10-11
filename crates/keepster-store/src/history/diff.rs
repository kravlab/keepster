//! What changed between two versions of a Root, computed where both versions are (ticket 46).
//!
//! The daemon holds both sides, so binary detection, rename detection and the size past which no hunks
//! are computed are decided here once, and every client shows the same answer.

use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use keepster_core::{CapturePolicy, RootPath, SegmentName, SnapshotId};

use super::{HistoryStore, SEGMENT_REFS, StoreError};

/// The size past which a file's versions are not compared line by line: 8 MiB, either side.
///
/// libgit2 inflates a blob whole to diff it, so this bounds the daemon's memory per request; it is
/// generous enough for a 60,000-line source file, the size ticket 09 found the web UI's editor still
/// renders smoothly. A larger file is reported as too large, never as a nonsense diff.
pub const DIFF_MAX_SIZE: u64 = 8 * 1024 * 1024;

/// What a Snapshot is compared with.
#[derive(Debug, Clone, Copy)]
pub enum DiffTarget<'a> {
    /// Another Snapshot.
    Snapshot(SnapshotId),
    /// The Working Tree as a Snapshot taken at `now` would hold it: under `policy`, so a file the
    /// Include Rules leave out is no change, and a file that cannot be read keeps its last content.
    WorkingTree {
        /// The Root's directory.
        root_path: &'a Path,
        /// What the Root captures.
        policy: &'a CapturePolicy,
        /// The moment of the comparison, which the stat filter's racy rule needs (ADR 0014).
        now: SystemTime,
    },
}

/// How a file differs between the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// Only on the newer side.
    Added,
    /// On both sides, different.
    Modified,
    /// Only on the older side.
    Deleted,
    /// Moved, perhaps edited too: one change, not a deletion and an addition.
    Renamed,
}

/// One file's part of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Where the file is on the newer side — or, for a deletion, where it was.
    pub path: RootPath,
    /// Where a renamed file was on the older side.
    pub old_path: Option<RootPath>,
    /// How it changed; `None` for a path asked about that did not change.
    pub change: Option<FileChange>,
    /// Whether either version is binary, by git's rule. Not probed for a file too large to diff.
    pub binary: bool,
    /// Whether either version is larger than [`DIFF_MAX_SIZE`].
    pub too_large: bool,
    /// The changed lines, in order; empty in a summary, and for a binary or too large file.
    pub hunks: Vec<Hunk>,
}

/// One run of changed lines with the context around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    /// The first line of the run on the older side, counting from 1.
    pub old_start: u32,
    /// How many lines of the older side the run covers.
    pub old_lines: u32,
    /// The first line of the run on the newer side, counting from 1.
    pub new_start: u32,
    /// How many lines of the newer side the run covers.
    pub new_lines: u32,
    /// The lines, context included.
    pub lines: Vec<DiffLine>,
}

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Whether it is context, or only on one side.
    pub kind: LineKind,
    /// Its bytes, without the line ending.
    pub text: Vec<u8>,
    /// Whether a newline ends it; `false` only for a file's last line, when the file ends without one.
    pub newline: bool,
}

/// Which side a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// On both sides.
    Context,
    /// Only on the newer side.
    Added,
    /// Only on the older side.
    Deleted,
}

/// Two trees compared, with renames found.
struct Compared<'store> {
    diff: git2::Diff<'store>,
    from: git2::Tree<'store>,
    to: git2::Tree<'store>,
}

impl HistoryStore {
    /// What changed from the Snapshot `from` to `to`: every changed file, in path order, without hunks —
    /// or, given a `path`, that one file with its hunks.
    ///
    /// A path is found by either of its names, so a renamed file asked about by where it was is answered
    /// with the rename. A path that did not change is answered as unchanged; one on neither side is not
    /// found.
    pub fn diff(
        &self,
        from: SnapshotId,
        to: &DiffTarget<'_>,
        path: Option<&RootPath>,
    ) -> Result<Vec<FileDiff>, StoreError> {
        let compared = self.compared(from, to)?;

        match path {
            None => compared
                .in_path_order()
                .into_iter()
                .map(|position| self.file_diff(&compared.diff, position, false))
                .collect(),
            Some(path) => match compared.position_of(path) {
                Some(position) => Ok(vec![self.file_diff(&compared.diff, position, true)?]),
                None => Ok(vec![self.unchanged(&compared, path)?]),
            },
        }
    }

    /// The same comparison as [`HistoryStore::diff`], as the unified patch git prints — every changed
    /// file's, or the one `path` names.
    ///
    /// A client that wants the text never reassembles it from hunks, so the bytes are the same for every
    /// client. A file too large to diff gets a line saying so where its hunks would be.
    pub fn unified(
        &self,
        from: SnapshotId,
        to: &DiffTarget<'_>,
        path: Option<&RootPath>,
    ) -> Result<Vec<u8>, StoreError> {
        let compared = self.compared(from, to)?;
        let positions = match path {
            None => compared.in_path_order(),
            Some(path) => match compared.position_of(path) {
                Some(position) => vec![position],
                // Unchanged: an empty patch — once the path is known to exist at all.
                None => {
                    self.unchanged(&compared, path)?;
                    Vec::new()
                }
            },
        };

        let mut text = Vec::new();
        for position in positions {
            let described = self.file_diff(&compared.diff, position, false)?;
            if described.too_large {
                let (old, new) = sides(&described);
                let _ = writeln!(text, "diff --git a/{old} b/{new}");
                let _ = writeln!(text, "File a/{old} or b/{new} is too large to diff");
                continue;
            }
            if let Some(mut patch) =
                git2::Patch::from_diff(&compared.diff, position).map_err(self.failed("read"))?
            {
                let buffer = patch.to_buf().map_err(self.failed("read"))?;
                text.extend_from_slice(&buffer);
            }
        }

        Ok(text)
    }

    /// The two trees, compared, with renames found.
    fn compared(&self, from: SnapshotId, to: &DiffTarget<'_>) -> Result<Compared<'_>, StoreError> {
        let from = self.snapshot_tree(from)?;
        let to = match *to {
            DiffTarget::Snapshot(snapshot) => self.snapshot_tree(snapshot)?,
            DiffTarget::WorkingTree {
                root_path,
                policy,
                now,
            } => self.working_tree(root_path, policy, now)?,
        };

        let mut options = git2::DiffOptions::new();
        options
            .include_typechange(true)
            // Past the threshold libgit2 does not load a blob at all, for hunks or for similarity.
            .max_size(i64::try_from(DIFF_MAX_SIZE).unwrap_or(i64::MAX));
        let mut diff = self
            .repository
            .diff_tree_to_tree(Some(&from), Some(&to), Some(&mut options))
            .map_err(self.failed("read"))?;
        // git's own defaults: a pair at least half similar is a rename, among at most a few hundred
        // candidates, so a large reorganisation degrades to deletions and additions rather than stalling.
        let mut find = git2::DiffFindOptions::new();
        find.renames(true);
        diff.find_similar(Some(&mut find))
            .map_err(self.failed("read"))?;

        Ok(Compared { diff, from, to })
    }

    /// The Working Tree as the next Snapshot would hold it, as a tree in this store.
    ///
    /// Built exactly as a capture builds it, from the newest Snapshot, with the evidence of the last pass
    /// vouching for unchanged files — but nothing is committed and the evidence is not updated: a diff is
    /// a question, and answering it must not change what the next capture compares against.
    fn working_tree(
        &self,
        root_path: &Path,
        policy: &CapturePolicy,
        now: SystemTime,
    ) -> Result<git2::Tree<'_>, StoreError> {
        let segment = SegmentName::hourly(now).map_err(StoreError::UnnameableMoment)?;
        let reference = format!("{SEGMENT_REFS}/{segment}");
        let (_, baseline) = self.parent_and_baseline(&segment, &reference)?;
        let baseline_tree = match &baseline {
            Some(commit) => Some(commit.tree().map_err(self.failed("read"))?),
            None => None,
        };
        let observations = self
            .index()
            .and_then(|index| index.observations())
            .unwrap_or_default();

        let (tree, _) = self.write_working_tree(
            baseline_tree.as_ref(),
            root_path,
            policy,
            &observations,
            now,
        )?;

        self.repository.find_tree(tree).map_err(self.failed("read"))
    }

    /// One delta as a file's part of the diff, with its hunks when `with_hunks`.
    fn file_diff(
        &self,
        diff: &git2::Diff<'_>,
        position: usize,
        with_hunks: bool,
    ) -> Result<FileDiff, StoreError> {
        let delta = diff
            .get_delta(position)
            .expect("a position taken from this very diff");
        let change = match delta.status() {
            git2::Delta::Added | git2::Delta::Copied => FileChange::Added,
            git2::Delta::Deleted => FileChange::Deleted,
            git2::Delta::Renamed => FileChange::Renamed,
            _ => FileChange::Modified,
        };
        let old = path_of(&delta.old_file());
        let new = path_of(&delta.new_file());
        let (path, old_path) = match change {
            FileChange::Deleted => (old, None),
            FileChange::Renamed => (new, Some(old)),
            _ => (new, None),
        };

        let blobs: Vec<git2::Oid> = [delta.old_file().id(), delta.new_file().id()]
            .into_iter()
            .filter(|blob| !blob.is_zero())
            .collect();
        let (binary, too_large) = self.classify(&blobs)?;

        let hunks = if with_hunks && !binary && !too_large {
            match git2::Patch::from_diff(diff, position).map_err(self.failed("read"))? {
                Some(patch) => self.hunks(&patch)?,
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };

        Ok(FileDiff {
            path,
            old_path,
            change: Some(change),
            binary,
            too_large,
            hunks,
        })
    }

    /// Whether any of `blobs` is binary, and whether any is too large to diff — the second decided
    /// first, so a large blob is never inflated to probe it.
    fn classify(&self, blobs: &[git2::Oid]) -> Result<(bool, bool), StoreError> {
        for blob in blobs {
            if self.blob_size(*blob)? > DIFF_MAX_SIZE {
                return Ok((false, true));
            }
        }
        for blob in blobs {
            if self.is_binary(*blob)? {
                return Ok((true, false));
            }
        }

        Ok((false, false))
    }

    fn hunks(&self, patch: &git2::Patch<'_>) -> Result<Vec<Hunk>, StoreError> {
        let mut hunks = Vec::with_capacity(patch.num_hunks());
        for number in 0..patch.num_hunks() {
            let (header, count) = patch.hunk(number).map_err(self.failed("read"))?;
            let mut lines = Vec::with_capacity(count);
            for line in 0..count {
                let line = patch
                    .line_in_hunk(number, line)
                    .map_err(self.failed("read"))?;
                let kind = match line.origin() {
                    ' ' => LineKind::Context,
                    '+' => LineKind::Added,
                    '-' => LineKind::Deleted,
                    // The "no newline at end of file" markers: what they say, the line's own bytes say.
                    _ => continue,
                };
                let content = line.content();
                let newline = content.ends_with(b"\n");
                let text = content.strip_suffix(b"\n").unwrap_or(content).to_vec();
                lines.push(DiffLine {
                    kind,
                    text,
                    newline,
                });
            }
            hunks.push(Hunk {
                old_start: header.old_start(),
                old_lines: header.old_lines(),
                new_start: header.new_start(),
                new_lines: header.new_lines(),
                lines,
            });
        }

        Ok(hunks)
    }

    /// A path no delta names: unchanged if it is a file on either side, and not found otherwise.
    fn unchanged(&self, compared: &Compared<'_>, path: &RootPath) -> Result<FileDiff, StoreError> {
        for tree in [&compared.to, &compared.from] {
            match tree.get_path(Path::new(path.as_str())) {
                Ok(entry) if entry.kind() == Some(git2::ObjectType::Blob) => {
                    let (binary, too_large) = self.classify(&[entry.id()])?;
                    return Ok(FileDiff {
                        path: path.clone(),
                        old_path: None,
                        change: None,
                        binary,
                        too_large,
                        hunks: Vec::new(),
                    });
                }
                Ok(_) => return Err(StoreError::NotAFile(path.clone())),
                Err(error) if error.code() == git2::ErrorCode::NotFound => {}
                Err(source) => return Err(self.failed("read")(source)),
            }
        }

        Err(StoreError::PathNotFound(path.clone()))
    }
}

impl Compared<'_> {
    /// Every delta's position, ordered by the path each is reported under.
    fn in_path_order(&self) -> Vec<usize> {
        let mut positions: Vec<(RootPath, usize)> = self
            .diff
            .deltas()
            .enumerate()
            .map(|(position, delta)| {
                let file = match delta.status() {
                    git2::Delta::Deleted => delta.old_file(),
                    _ => delta.new_file(),
                };
                (path_of(&file), position)
            })
            .collect();
        positions.sort();

        positions
            .into_iter()
            .map(|(_, position)| position)
            .collect()
    }

    /// The delta that names `path` on its newer side — or, failing that, on its older side.
    fn position_of(&self, path: &RootPath) -> Option<usize> {
        let wanted = Some(path.as_str().as_bytes());

        self.diff
            .deltas()
            .position(|delta| delta.new_file().path_bytes() == wanted)
            .or_else(|| {
                self.diff
                    .deltas()
                    .position(|delta| delta.old_file().path_bytes() == wanted)
            })
    }
}

/// The path a diff file names, as a Root-relative path.
///
/// Every path in a store was written from a [`RootPath`], so one that does not parse back is a store
/// written by something else; it is shown as the Root itself rather than refused, since a diff is read
/// only to be looked at.
fn path_of(file: &git2::DiffFile<'_>) -> RootPath {
    file.path_bytes()
        .map(String::from_utf8_lossy)
        .and_then(|path| RootPath::parse(&path).ok())
        .unwrap_or_else(RootPath::root)
}

/// The two names a too-large file goes by in a patch header.
fn sides(file: &FileDiff) -> (&str, &str) {
    let new = file.path.as_str();
    let old = file.old_path.as_ref().map_or(new, RootPath::as_str);

    (old, new)
}
