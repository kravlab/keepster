//! Export's two halves that touch the disk (ticket 58, ticket 19): what is where each file of a Snapshot
//! would go in a directory the request names, and the writes themselves — and the selection a download
//! streams, with its size known first.
//!
//! The same read as Restore, written somewhere else: the versions are found as Restore finds them, but a
//! Withheld path asked for by name is passed over rather than refused, nothing on the way is followed, and
//! what the Safety Snapshot could not hold is asked about only where the destination is inside a Root.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;

use keepster_core::{
    Ancestry, CapturePolicy, Existing, Found, ModeChoice, Modes, Naming, PathPlan, Plan, RootPath,
    SnapshotId, Step, Unrecoverable, WithheldReason,
};

use super::restore::{
    Bits, Preflight, Source, TEMPORARY, Version, Written, absolute, executable, recoverability_in,
    set_mode, still_as_planned, sync_directory, variant_of,
};
use super::{Content, HistoryStore, StoreError, permissions};

/// What an Export covers and where it writes.
#[derive(Debug, Clone, Copy)]
pub struct ExportScope<'a> {
    /// The Snapshot whose versions are written.
    pub from: SnapshotId,
    /// The paths asked for — a directory meaning everything beneath it — or every file when empty.
    pub paths: &'a [RootPath],
    /// The directory written into, each file at its Snapshot path beneath it.
    pub dest: &'a Path,
    /// What the destination makes of names.
    pub naming: Naming,
    /// Where a renamed file is written instead of its own path.
    pub renamed: &'a BTreeMap<RootPath, RootPath>,
    /// The Watched Root the destination lies inside, if it does.
    pub within: Option<Within<'a>>,
}

/// A Watched Root an Export writes into: what its Safety Snapshot can hold of what is overwritten.
#[derive(Debug, Clone, Copy)]
pub struct Within<'a> {
    /// The Root's directory.
    pub root_path: &'a Path,
    /// What the Root captures.
    pub policy: &'a CapturePolicy,
    /// Where the destination is in it.
    pub at: &'a RootPath,
}

/// One file of a selection, as a download streams it.
#[derive(Debug, Clone)]
pub struct Selected {
    /// Its path in the Snapshot.
    pub path: RootPath,
    /// How many bytes it holds — a link's target text, for a link.
    pub size: u64,
    /// Its permission bits, where the Snapshot observed them.
    pub mode: Option<u32>,
    /// Whether its tree mode is executable.
    pub executable: bool,
    /// Whether it is a symbolic link.
    pub link: bool,
    /// Its content, which only the store reads.
    blob: git2::Oid,
}

/// Everything a selection of a Snapshot holds: the files, and the paths it holds no version of.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// Every file, in path order.
    pub files: Vec<Selected>,
    /// Every Withheld path, with why: absent from any export, and said so.
    pub withheld: Vec<(RootPath, WithheldReason)>,
}

impl HistoryStore {
    /// The files of the Snapshot `from` under `paths` — the whole Snapshot when there are none — and the
    /// Withheld paths among them, a Withheld path asked for by name included.
    pub fn selection(&self, from: SnapshotId, paths: &[RootPath]) -> Result<Selection, StoreError> {
        let mut selection = Selection::default();
        for (path, version) in self.versions(from, paths, true)? {
            match version {
                Version::Held(source) => selection.files.push(Selected {
                    path,
                    size: source.size,
                    mode: source.mode,
                    executable: source.executable,
                    link: source.link,
                    blob: source.blob,
                }),
                Version::Withheld(reason) => selection.withheld.push((path, reason)),
            }
        }

        Ok(selection)
    }

    /// Hand the bytes of `selected` to `sink`, a chunk at a time.
    pub fn read_selected(
        &self,
        selected: &Selected,
        sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
    ) -> Result<(), StoreError> {
        Content {
            store: self,
            blob: selected.blob,
            size: selected.size,
        }
        .read(0, selected.size, sink)
    }

    /// The files `scope` covers in its Snapshot, and what is in the destination where each would go.
    pub fn export_preflight(&self, scope: &ExportScope<'_>) -> Result<Preflight, StoreError> {
        let versions = self.versions(scope.from, scope.paths, true)?;
        let mut found = Vec::with_capacity(versions.len());
        let mut sources = Vec::with_capacity(versions.len());
        for (path, version) in versions {
            let target = scope
                .renamed
                .get(&path)
                .cloned()
                .unwrap_or_else(|| path.clone());
            match version {
                Version::Held(source) => {
                    found.push(facts(scope, &path, &target, &source));
                    sources.push(Some(source));
                }
                // Nothing to write, so nothing in the destination to look at.
                Version::Withheld(reason) => {
                    found.push(Found {
                        path,
                        target,
                        ancestry: Ancestry::Present,
                        existing: Existing::Absent,
                        case_variant: None,
                        withheld: Some(reason),
                        modes: None,
                    });
                    sources.push(None);
                }
            }
        }

        Ok(Preflight { found, sources })
    }

    /// Write what `plan` says, path by path, each re-checked first, saying after each how many are done:
    /// a path whose facts changed since the preflight fails rather than being written over something
    /// nobody was asked about.
    ///
    /// Each file is written beside its destination and renamed into place, so no reader ever sees half of
    /// it, and a failure leaves what was there.
    pub fn export_write(
        &self,
        scope: &ExportScope<'_>,
        preflight: &Preflight,
        plan: &Plan,
        done: &mut dyn FnMut(u64),
    ) -> Vec<Written> {
        let mut written = Vec::with_capacity(plan.paths.len());
        for ((planned, before), source) in plan
            .paths
            .iter()
            .zip(&preflight.found)
            .zip(&preflight.sources)
        {
            written.push(self.export_one(scope, planned, before, source.as_ref()));
            done(written.len() as u64);
        }

        written
    }

    /// One path as the plan says.
    fn export_one(
        &self,
        scope: &ExportScope<'_>,
        planned: &PathPlan,
        before: &Found,
        source: Option<&Source>,
    ) -> Written {
        match planned.step {
            Step::Unchanged => return Written::Unchanged,
            Step::Skipped | Step::Blocked => return Written::Skipped,
            Step::Restore | Step::Create => {}
        }
        let Some(source) = source else {
            return Written::Skipped;
        };
        let now = facts(scope, &before.path, &before.target, source);
        if !still_as_planned(before, &now) {
            return Written::Failed("it changed between the preflight and the write".to_owned());
        }
        // The content is already the version's: only its mode is set.
        let outcome = match (&now.existing, source.mode) {
            (Existing::Same, Some(mode)) => {
                super::open_unfollowed(&absolute(scope.dest, &planned.target))
                    .and_then(|file| set_mode(&file, Bits::Exactly(mode)))
            }
            _ => self.write_exported(scope, planned, &now, source),
        };
        match outcome {
            Ok(()) if planned.step == Step::Create => Written::Created,
            Ok(()) => Written::Restored,
            Err(error) => Written::Failed(error.to_string()),
        }
    }

    /// Write one file into the destination, acting on what is there now — which an earlier file of the
    /// same request may have changed.
    fn write_exported(
        &self,
        scope: &ExportScope<'_>,
        planned: &PathPlan,
        found: &Found,
        source: &Source,
    ) -> io::Result<()> {
        let dest = scope.dest;
        let target = absolute(dest, &planned.target);
        let parent = target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| dest.to_path_buf());

        match &found.ancestry {
            Ancestry::Present => {}
            Ancestry::Missing => fs::create_dir_all(&parent)?,
            // Confirmed: the file in the way becomes a directory.
            Ancestry::File { at, .. } => {
                fs::remove_file(absolute(dest, at))?;
                fs::create_dir_all(&parent)?;
            }
            Ancestry::Link { .. } => {
                return Err(io::Error::other("a directory above it is a link"));
            }
        }

        let replaced = match &planned.replaces {
            Some(replaces) => absolute(dest, replaces),
            None => target.clone(),
        };
        let bits = match (planned.modes, found.modes, source.mode) {
            // The bits the user kept are the old file's, but setuid and setgid were granted to the old
            // content, not to this.
            (Some(ModeChoice::Keep), Some(Modes { current, .. }), _) => {
                Bits::Exactly(current & !0o6000)
            }
            (_, _, Some(mode)) => Bits::Exactly(mode),
            // A Snapshot that observed no modes has the executable bit alone to give, applied to the
            // umask's bits.
            (_, _, None) => Bits::Executable {
                base: None,
                executable: source.executable,
            },
        };

        // Short, whatever the file's own name: a long name plus a suffix could exceed what the
        // filesystem allows in one component.
        let temporary = parent.join(format!(
            ".keepster-export.{}.{}",
            std::process::id(),
            TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let written = if source.link {
            self.link_temporary(&temporary, source)
        } else {
            self.write_temporary(&temporary, source, bits)
        };
        if let Err(error) = written.and_then(|()| fs::rename(&temporary, &replaced)) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        // A name in another case was replaced: the file now takes the Snapshot's spelling.
        if replaced != target {
            fs::rename(&replaced, &target).map_err(|error| {
                io::Error::other(format!(
                    "the content is in place as {}, but the name could not take the Snapshot's \
                     spelling: {error}",
                    replaced.display()
                ))
            })?;
        }
        sync_directory(&parent);

        Ok(())
    }
}

/// What is in the destination where the file `path` of the Snapshot would go, at `target`.
fn facts(scope: &ExportScope<'_>, path: &RootPath, target: &RootPath, source: &Source) -> Found {
    let dest = scope.dest;

    // A destination not made yet is made with the first file's directories.
    let mut ancestry = if dest.is_dir() {
        Ancestry::Present
    } else {
        Ancestry::Missing
    };
    for ancestor in target.ancestors().filter(|_| dest.is_dir()) {
        match fs::symlink_metadata(absolute(dest, &ancestor)) {
            Err(_) => {
                ancestry = Ancestry::Missing;
                break;
            }
            // An Export follows nothing: a link on the way would lead the write out of what was checked.
            Ok(metadata) if metadata.file_type().is_symlink() => {
                ancestry = Ancestry::Link { at: ancestor };
                break;
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(metadata) => {
                let unrecoverable = unrecoverable(scope, &ancestor, &metadata);
                ancestry = Ancestry::File {
                    at: ancestor,
                    unrecoverable,
                };
                break;
            }
        }
    }

    let case_variant = match ancestry {
        Ancestry::Present if scope.naming.folds_case || scope.naming.folds_normalization => {
            variant_of(dest, target, scope.naming)
        }
        _ => None,
    };
    let (existing, modes) = match &ancestry {
        Ancestry::Present => existing(scope, target, source),
        _ => (Existing::Absent, None),
    };

    Found {
        path: path.clone(),
        target: target.clone(),
        ancestry,
        existing,
        case_variant,
        withheld: None,
        modes,
    }
}

/// What is at `target` in the destination, compared with the version to write, and the permission bits
/// of a file there where they and the version's differ.
fn existing(
    scope: &ExportScope<'_>,
    target: &RootPath,
    source: &Source,
) -> (Existing, Option<Modes>) {
    let at = absolute(scope.dest, target);
    let Ok(metadata) = fs::symlink_metadata(&at) else {
        return (Existing::Absent, None);
    };

    if metadata.file_type().is_symlink() {
        let same = source.link
            && fs::read_link(&at).is_ok_and(|text| {
                git2::Oid::hash_object(git2::ObjectType::Blob, &super::link_text(&text))
                    .is_ok_and(|current| current == source.blob)
            });
        if same {
            return (Existing::Same, None);
        }
        let unrecoverable = unrecoverable(scope, target, &metadata);
        // One link replaced by another, in place: no change of kind.
        return if source.link {
            (
                Existing::File {
                    links: 1,
                    links_inside: Vec::new(),
                    unrecoverable,
                },
                None,
            )
        } else {
            (Existing::Other { unrecoverable }, None)
        };
    }
    if metadata.is_dir() {
        return (Existing::Directory, None);
    }
    if !metadata.is_file() || source.link {
        let unrecoverable = unrecoverable(scope, target, &metadata);
        return (Existing::Other { unrecoverable }, None);
    }

    // The content alone where the Snapshot observed modes: one that differs is a question of its own
    // (ticket 17). Otherwise the executable bit is part of the version, where the platform keeps it.
    let modes = source.mode.and_then(|snapshot| {
        let current = permissions(&metadata)?;
        (current != snapshot).then_some(Modes { snapshot, current })
    });
    let same_content = git2::Oid::hash_file(git2::ObjectType::Blob, &at)
        .is_ok_and(|current| current == source.blob);
    let same_bit = source.mode.is_some()
        || executable(&metadata).is_none_or(|current| current == source.executable);
    if same_content && same_bit {
        return (Existing::Same, modes);
    }

    let unrecoverable = unrecoverable(scope, target, &metadata);
    (
        Existing::File {
            links: 1,
            links_inside: Vec::new(),
            unrecoverable,
        },
        modes,
    )
}

/// Why what is at `target` could not be brought back once overwritten, where the destination is inside a
/// Root whose Safety Snapshot is to hold it — and nothing anywhere else, where nothing would.
fn unrecoverable(
    scope: &ExportScope<'_>,
    target: &RootPath,
    metadata: &fs::Metadata,
) -> Option<Unrecoverable> {
    let within = scope.within?;
    if !metadata.is_file() && !metadata.file_type().is_symlink() {
        return Some(Unrecoverable::Unsupported);
    }
    let in_root = joined(within.at, target);
    recoverability_in(within.root_path, within.policy, &in_root, metadata).or_else(|| {
        (metadata.is_file() && File::open(absolute(scope.dest, target)).is_err())
            .then_some(Unrecoverable::Unreadable)
    })
}

/// `path` beneath `directory`, both Root-relative.
fn joined(directory: &RootPath, path: &RootPath) -> RootPath {
    if directory.is_root() {
        return path.clone();
    }

    path.as_bytes()
        .split(|byte| *byte == b'/')
        .fold(directory.clone(), |joined, name| joined.child(name))
}
