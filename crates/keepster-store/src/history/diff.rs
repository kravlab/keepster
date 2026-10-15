//! What changed from a Snapshot to another Snapshot or the Working Tree, computed where both sides
//! are (ticket 46).
//!
//! The daemon holds both sides, so binary detection, rename detection and the size past which no hunks
//! are computed are decided here once, and every client shows the same answer.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::time::SystemTime;

use keepster_core::{CapturePolicy, ModeKind, ModeTable, Record, RootPath, SIDECAR, SnapshotId};

use super::{
    HistoryStore, IncompletePath, ModeChange, StoreError, WithheldReason, filesystem_path,
    mode_changes, mode_kind,
};

/// The size past which a file's versions are not compared line by line: 8 MiB, either side.
///
/// libgit2 inflates a blob whole to diff it, so this bounds the daemon's memory per file compared — a
/// whole patch of many files is as large as their patches together; it is generous enough for a
/// 60,000-line source file, the size ticket 09 found the web UI's editor still renders smoothly. A
/// larger file is reported as too large, never as a nonsense diff.
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

/// A diff: the files, and — when the newer side is the Working Tree — what of it could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// Every changed file in path order, or the one asked for.
    pub files: Vec<FileDiff>,
    /// Working Tree files that could not be read as they are, which keep their last content as a
    /// capture keeps it: the comparison is not whole, and says where and why.
    pub incomplete: Vec<IncompletePath>,
}

/// One file's part of a diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// Where the file is on the newer side — or, for a deletion, where it was. A directory is named
    /// only when its mode is all that changed there.
    pub path: RootPath,
    /// Where a renamed file was on the older side.
    pub old_path: Option<RootPath>,
    /// How it changed; `None` for a path asked about that did not change.
    pub change: Option<FileChange>,
    /// Whether either version is binary, by git's rule. Not probed for a file too large to diff.
    pub binary: bool,
    /// Whether either version is larger than [`DIFF_MAX_SIZE`].
    pub too_large: bool,
    /// Why the next Snapshot would not hold a file that is still in the Working Tree — which is why it
    /// reads as deleted, and is not (ADR 0009).
    pub withheld: Option<WithheldReason>,
    /// The file's permission bits on each side, all twelve, when they changed between them — alone, or
    /// with the content (ticket 50).
    pub old_mode: Option<u32>,
    /// See [`FileDiff::old_mode`].
    pub new_mode: Option<u32>,
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
    /// The modes each side holds.
    modes: (ModeTable, ModeTable),
    /// Files and directories whose mode changed and nothing else, which git sees no change in.
    mode_only: Vec<ModeChange>,
    /// What the walk of the Working Tree found it could not hold, when it is the newer side.
    working: Working,
}

/// One changed file of a comparison.
#[derive(Debug, Clone, Copy)]
enum Item {
    /// The delta at this position of the diff.
    Delta(usize),
    /// The mode-only change at this position.
    ModeOnly(usize),
}

/// What a walk of the Working Tree left out of the tree it built.
#[derive(Default)]
struct Working {
    withheld: HashMap<RootPath, WithheldReason>,
    incomplete: Vec<IncompletePath>,
}

impl Working {
    /// Why the newer side withholds `path`: itself, or a directory above it withheld whole.
    fn withheld_at(&self, path: &RootPath) -> Option<WithheldReason> {
        std::iter::once(path.clone())
            .chain(path.ancestors())
            .find_map(|candidate| self.withheld.get(&candidate).copied())
    }
}

/// Which changes a request is about.
enum Selection {
    /// These, in this order.
    Items(Vec<Item>),
    /// A path no delta names, which is a file on one side or the other.
    Unchanged(FileDiff),
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
    ) -> Result<Comparison, StoreError> {
        let compared = self.compared(from, to)?;
        let files = match self.select(&compared, path)? {
            Selection::Items(items) => items
                .into_iter()
                .map(|item| self.file_diff(&compared, item, path.is_some()))
                .collect::<Result<_, _>>()?,
            Selection::Unchanged(file) => vec![file],
        };

        Ok(Comparison {
            files,
            incomplete: compared.working.incomplete,
        })
    }

    /// The same comparison as [`HistoryStore::diff`], as the unified patch git prints — every changed
    /// file's, or the one `path` names; empty for a path that did not change.
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
        let Selection::Items(items) = self.select(&compared, path)? else {
            return Ok(Vec::new());
        };

        let mut text = Vec::new();
        for item in items {
            let described = self.file_diff(&compared, item, false)?;
            let (old, new) = sides(&described);
            let position = match item {
                Item::Delta(position) => position,
                Item::ModeOnly(_) => {
                    let _ = writeln!(text, "diff --git {old} {new}");
                    text.extend_from_slice(&mode_lines(&described));
                    continue;
                }
            };
            if described.too_large {
                let _ = writeln!(text, "diff --git {old} {new}");
                text.extend_from_slice(&mode_lines(&described));
                let _ = writeln!(text, "File {old} or {new} is too large to diff");
                continue;
            }
            if let Some(mut patch) =
                git2::Patch::from_diff(&compared.diff, position).map_err(self.failed("read"))?
            {
                let buffer = patch.to_buf().map_err(self.failed("read"))?;
                text.extend_from_slice(&with_modes(&buffer, &described));
            }
        }

        Ok(text)
    }

    /// The deltas `path` is about: every one, in path order, without it; the one that names it with it —
    /// or, when none does, the file it names, unchanged.
    fn select(
        &self,
        compared: &Compared<'_>,
        path: Option<&RootPath>,
    ) -> Result<Selection, StoreError> {
        Ok(match path {
            None => Selection::Items(compared.in_path_order()),
            Some(path) => match compared.item_of(path) {
                Some(item) => Selection::Items(vec![item]),
                None => Selection::Unchanged(self.unchanged(compared, path)?),
            },
        })
    }

    /// The two trees, compared, with renames found.
    fn compared(&self, from: SnapshotId, to: &DiffTarget<'_>) -> Result<Compared<'_>, StoreError> {
        let from = self.snapshot_tree(from)?;
        let from_modes = self.sidecar(&from)?.sidecar.modes();
        let (to, working) = match *to {
            DiffTarget::Snapshot(snapshot) => {
                let tree = self.snapshot_tree(snapshot)?;
                // What the newer Snapshot says it withheld, so a file that became Withheld reads as
                // that and not as deleted — exactly as against the Working Tree.
                let withheld = self
                    .sidecar(&tree)?
                    .sidecar
                    .records
                    .into_iter()
                    .filter_map(|record| match record {
                        Record::Withheld { path, reason } => Some((path, reason)),
                        Record::Incomplete { .. } | Record::Mode { .. } => None,
                    })
                    .collect();
                (
                    tree,
                    Working {
                        withheld,
                        incomplete: Vec::new(),
                    },
                )
            }
            DiffTarget::WorkingTree {
                root_path,
                policy,
                now,
            } => self.working_tree(root_path, policy, now)?,
        };
        let to_modes = self.sidecar(&to)?.sidecar.modes();
        // The sidecar is the Snapshot's own record, never a file of the Root, so neither side has it:
        // taking it out before comparing also keeps it from ever pairing with a file as a rename.
        let from = self.without_sidecar(from)?;
        let to = self.without_sidecar(to)?;

        let mut options = git2::DiffOptions::new();
        options
            .include_typechange(true)
            // Past the threshold libgit2 loads no blob for hunks.
            .max_size(i64::try_from(DIFF_MAX_SIZE).unwrap_or(i64::MAX));
        let mut diff = self
            .repository
            .diff_tree_to_tree(Some(&from), Some(&to), Some(&mut options))
            .map_err(self.failed("read"))?;
        // git's own defaults: a pair at least half similar is a rename, among at most a thousand
        // candidates, so a large reorganisation degrades to deletions and additions rather than stalling.
        // Measuring similarity reads every candidate whole, whatever its size, so when one is too large to
        // diff, renames are found by identical content alone, which needs no read at all.
        let mut find = git2::DiffFindOptions::new();
        find.renames(true)
            .exact_match_only(self.any_candidate_too_large(&diff)?);
        diff.find_similar(Some(&mut find))
            .map_err(self.failed("read"))?;

        // A file whose content and tree mode are as they were has no delta, whatever its permission
        // bits did; those are compared from the sidecars.
        let named: std::collections::HashSet<&[u8]> = diff
            .deltas()
            .filter_map(|delta| delta.new_file().path_bytes())
            .collect();
        let mode_only = mode_changes(&self.repository, &from, &from_modes, &to, &to_modes)
            .into_iter()
            .filter(|change| !named.contains(change.path.as_bytes()))
            .collect();

        Ok(Compared {
            diff,
            from,
            to,
            modes: (from_modes, to_modes),
            mode_only,
            working,
        })
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
    ) -> Result<(git2::Tree<'_>, Working), StoreError> {
        let segment = self.segment_at(now)?;
        let reference = super::retention::segment_ref(&segment);
        let (_, baseline) = self.parent_and_baseline(&segment, &reference)?;
        let baseline_tree = match &baseline {
            Some(commit) => Some(commit.tree().map_err(self.failed("read"))?),
            None => None,
        };
        let observations = self
            .index()
            .and_then(|index| index.observations())
            .unwrap_or_default();

        // Whether the header is counted again changes how the modes are written, never what they are.
        let (tree, walked) = self.write_working_tree(
            baseline_tree.as_ref(),
            root_path,
            policy,
            &observations,
            now,
            false,
        )?;
        let working = Working {
            withheld: walked
                .withheld
                .into_iter()
                .map(|withheld| (withheld.path, withheld.reason))
                .collect(),
            incomplete: walked.incomplete,
        };
        let tree = self
            .repository
            .find_tree(tree)
            .map_err(self.failed("read"))?;

        Ok((tree, working))
    }

    /// `tree` without the sidecar at its root — `tree` itself when it has none, which is most Snapshots.
    fn without_sidecar<'store>(
        &'store self,
        tree: git2::Tree<'store>,
    ) -> Result<git2::Tree<'store>, StoreError> {
        if tree.get_name(SIDECAR).is_none() {
            return Ok(tree);
        }

        let mut builder = self
            .repository
            .treebuilder(Some(&tree))
            .map_err(self.failed("read"))?;
        builder.remove(SIDECAR).map_err(self.failed("written"))?;
        let stripped = builder.write().map_err(self.failed("written"))?;

        self.repository
            .find_tree(stripped)
            .map_err(self.failed("read"))
    }

    /// Whether an added or deleted file — a rename candidate — is larger than the diff threshold.
    fn any_candidate_too_large(&self, diff: &git2::Diff<'_>) -> Result<bool, StoreError> {
        for delta in diff.deltas() {
            let blob = match delta.status() {
                git2::Delta::Added => delta.new_file().id(),
                git2::Delta::Deleted => delta.old_file().id(),
                _ => continue,
            };
            if self.blob_size(blob)? > DIFF_MAX_SIZE {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// One change as a file's part of the diff, with its hunks when `with_hunks`.
    fn file_diff(
        &self,
        compared: &Compared<'_>,
        item: Item,
        with_hunks: bool,
    ) -> Result<FileDiff, StoreError> {
        let position = match item {
            Item::Delta(position) => position,
            Item::ModeOnly(at) => {
                let change = &compared.mode_only[at];
                let (binary, too_large) = match change.kind {
                    ModeKind::Directory => (false, false),
                    _ => {
                        let entry = compared
                            .to
                            .get_path(&filesystem_path(change.path.as_bytes()))
                            .map_err(self.failed("read"))?;
                        self.classify(&[entry.id()])?
                    }
                };
                return Ok(FileDiff {
                    path: change.path.clone(),
                    old_path: None,
                    change: Some(FileChange::Modified),
                    binary,
                    too_large,
                    withheld: None,
                    old_mode: Some(change.old),
                    new_mode: Some(change.new),
                    hunks: Vec::new(),
                });
            }
        };
        let diff = &compared.diff;
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
        // Every bit where both sides observed them; the executable bit alone, as the tree modes carry
        // it, where either did not.
        let modes = mode_in(&compared.modes.0, &old, delta.old_file().mode())
            .zip(mode_in(&compared.modes.1, &new, delta.new_file().mode()))
            .or_else(|| {
                regular_mode(delta.old_file().mode()).zip(regular_mode(delta.new_file().mode()))
            });
        let (old_mode, new_mode) = match modes {
            Some((before, after)) if before != after => (Some(before), Some(after)),
            _ => (None, None),
        };
        let (path, old_path) = match change {
            FileChange::Deleted => (old, None),
            FileChange::Renamed => (new, Some(old)),
            _ => (new, None),
        };
        let withheld = match change {
            FileChange::Deleted => compared.working.withheld_at(&path),
            _ => None,
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
            withheld,
            old_mode,
            new_mode,
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

    /// A patch's hunks, line by line, with each line's ending taken off and remembered.
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
            match tree.get_path(&filesystem_path(path.as_bytes())) {
                Ok(entry) if entry.kind() == Some(git2::ObjectType::Blob) => {
                    let (binary, too_large) = self.classify(&[entry.id()])?;
                    return Ok(FileDiff {
                        path: path.clone(),
                        old_path: None,
                        change: None,
                        binary,
                        too_large,
                        withheld: None,
                        old_mode: None,
                        new_mode: None,
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
    /// Every change, ordered by the path each is reported under.
    fn in_path_order(&self) -> Vec<Item> {
        let mut items: Vec<(RootPath, Item)> = self
            .diff
            .deltas()
            .enumerate()
            .map(|(position, delta)| {
                let file = match delta.status() {
                    git2::Delta::Deleted => delta.old_file(),
                    _ => delta.new_file(),
                };
                (path_of(&file), Item::Delta(position))
            })
            .chain(
                self.mode_only
                    .iter()
                    .enumerate()
                    .map(|(at, change)| (change.path.clone(), Item::ModeOnly(at))),
            )
            .collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));

        items.into_iter().map(|(_, item)| item).collect()
    }

    /// The change that names `path`: a delta on its newer side — or, failing that, on its older side —
    /// or a change of its mode alone.
    fn item_of(&self, path: &RootPath) -> Option<Item> {
        let wanted = Some(path.as_bytes());

        self.diff
            .deltas()
            .position(|delta| delta.new_file().path_bytes() == wanted)
            .or_else(|| {
                self.diff
                    .deltas()
                    .position(|delta| delta.old_file().path_bytes() == wanted)
            })
            .map(Item::Delta)
            .or_else(|| {
                self.mode_only
                    .iter()
                    .position(|change| change.path == *path)
                    .map(Item::ModeOnly)
            })
    }
}

/// The path a diff file names, as a Root-relative path, by its exact bytes.
///
/// Every path in a store was written from a [`RootPath`], so one that does not parse back is a store
/// written by something else; it is shown as the Root itself rather than refused, since a diff is read
/// only to be looked at.
fn path_of(file: &git2::DiffFile<'_>) -> RootPath {
    file.path_bytes()
        .and_then(|path| RootPath::parse_bytes(path).ok())
        .unwrap_or_else(RootPath::root)
}

/// A regular file's permission bits as `modes` holds them, or `None` for anything that is not a
/// regular file and where the Snapshot observed none.
fn mode_in(modes: &ModeTable, path: &RootPath, mode: git2::FileMode) -> Option<u32> {
    match mode_kind(i32::from(mode))? {
        ModeKind::Directory => None,
        kind => modes.mode_of(path, kind),
    }
}

/// A regular file's tree mode as permission bits, `0o644` or `0o755`, or `None` for anything that is
/// not a regular file.
fn regular_mode(mode: git2::FileMode) -> Option<u32> {
    match mode {
        git2::FileMode::Blob => Some(0o644),
        git2::FileMode::BlobExecutable => Some(0o755),
        _ => None,
    }
}

/// The lines git writes for a change of mode, with all twelve bits where it writes only the
/// executable one; nothing when the mode did not change.
fn mode_lines(file: &FileDiff) -> Vec<u8> {
    match (file.old_mode, file.new_mode) {
        (Some(old), Some(new)) => {
            format!("old mode 10{old:04o}\nnew mode 10{new:04o}\n").into_bytes()
        }
        _ => Vec::new(),
    }
}

/// git's patch for one file with its mode lines replaced by [`mode_lines`]: git knows only the
/// executable bit, so it writes none for a change of any other, and the wrong bits for that one.
fn with_modes(patch: &[u8], file: &FileDiff) -> Vec<u8> {
    let mut lines = patch.split_inclusive(|byte| *byte == b'\n');
    let mut text = Vec::with_capacity(patch.len());
    if let Some(first) = lines.next() {
        text.extend_from_slice(first);
    }
    text.extend_from_slice(&mode_lines(file));
    let mut header = true;
    for line in lines {
        // The mode lines come straight after the first, before the `index` line.
        if header && (line.starts_with(b"old mode ") || line.starts_with(b"new mode ")) {
            continue;
        }
        header = false;
        text.extend_from_slice(line);
    }

    text
}

/// The two names a file goes by in a patch header this code writes, `a/` and `b/` prefixed and quoted
/// as libgit2 quotes the headers it writes, so a name reads the same in every header of one patch.
fn sides(file: &FileDiff) -> (String, String) {
    let new = &file.path;
    let old = file.old_path.as_ref().unwrap_or(new);

    (header_name("a/", old), header_name("b/", new))
}

/// `prefix` and `path` as one name of a patch header: as they are, or — where a byte is a quote, a
/// backslash, a control or anything beyond ASCII, a name that is not UTF-8 among them — in double
/// quotes with that byte escaped, as libgit2 and git write such a name (ticket 74).
fn header_name(prefix: &str, path: &RootPath) -> String {
    let mut name = prefix.as_bytes().to_vec();
    name.extend_from_slice(path.as_bytes());
    if !name
        .iter()
        .any(|byte| matches!(byte, b'"' | b'\\') || !(b' '..=b'~').contains(byte))
    {
        return String::from_utf8_lossy(&name).into_owned();
    }

    let mut quoted = String::from("\"");
    for byte in name {
        match byte {
            b'\x07' => quoted.push_str("\\a"),
            b'\x08' => quoted.push_str("\\b"),
            b'\t' => quoted.push_str("\\t"),
            b'\n' => quoted.push_str("\\n"),
            b'\x0b' => quoted.push_str("\\v"),
            b'\x0c' => quoted.push_str("\\f"),
            b'\r' => quoted.push_str("\\r"),
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            b' '..=b'~' => quoted.push(char::from(byte)),
            other => quoted.push_str(&format!("\\{other:03o}")),
        }
    }
    quoted.push('"');

    quoted
}
