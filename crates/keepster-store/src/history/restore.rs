//! Restore's two halves that touch the disk (ticket 47): what is where each file of a Snapshot would go,
//! and the writes themselves. What the facts mean is decided in `keepster_core::plan`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use keepster_core::{
    Ancestry, CapturePolicy, Existing, Found, IGNORE_FILE, Kind, ModeChoice, ModeKind, ModeTable,
    Modes, Naming, Plan, Record, RootPath, SnapshotId, Step, Unrecoverable, Verdict,
    WithheldReason, ignore_file_rules,
};

use super::{
    Content, HistoryStore, StoreError, WirePath, is_beneath, mode_kind, nothing_at, permissions,
    withheld_at,
};

/// What a Restore covers and where it writes.
#[derive(Debug, Clone, Copy)]
pub struct RestoreScope<'a> {
    /// The Snapshot whose versions are written.
    pub from: SnapshotId,
    /// The paths asked for — a directory meaning everything beneath it — or every file when empty.
    pub paths: &'a [RootPath],
    /// The Root's directory.
    pub root_path: &'a Path,
    /// What the Root captures: what the Safety Snapshot can hold of what is overwritten.
    pub policy: &'a CapturePolicy,
    /// What the destination makes of names.
    pub naming: Naming,
    /// Where a renamed file is written instead of its own path.
    pub renamed: &'a BTreeMap<RootPath, RootPath>,
    /// How this platform lists every name of a file, where std cannot — Windows, whose call lives with
    /// the rest of the workspace's `unsafe` in `keepster-service`. `None` uses `(dev, ino)` on Unix.
    pub link_names: Option<LinkNames>,
}

/// Every name of the file at a path, as full paths.
pub type LinkNames = fn(&Path) -> io::Result<Vec<PathBuf>>;

/// The facts a plan is made from, with the versions to write.
#[derive(Debug, Clone)]
pub struct Preflight {
    /// What is on disk where each file would go, in path order.
    pub found: Vec<Found>,
    /// The version of each, or `None` for a Withheld path, which has none.
    pub(super) sources: Vec<Option<Source>>,
}

impl Preflight {
    /// How many files have a version to write, and their sizes summed: the selection, Withheld paths
    /// aside.
    pub fn held(&self) -> (u64, u64) {
        self.sources
            .iter()
            .flatten()
            .fold((0, 0), |(files, bytes), source| {
                (files + 1, bytes.saturating_add(source.size))
            })
    }
}

/// What happened to one path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    /// A file in place was replaced.
    Restored,
    /// The file was created.
    Created,
    /// The file already held the version.
    Unchanged,
    /// Passed over, as the plan said.
    Skipped,
    /// It could not be written, and why. What was there is left as it was, unless the reason says the
    /// content is in place under the name that was there.
    Failed(String),
}

/// One file of the Snapshot.
#[derive(Debug, Clone)]
pub(super) struct Source {
    /// Its content.
    pub(super) blob: git2::Oid,
    /// How many bytes its content holds.
    pub(super) size: u64,
    /// Its permission bits, all twelve, as the Snapshot holds them (ticket 50) — `None` where it
    /// observed none.
    pub(super) mode: Option<u32>,
    /// Whether its tree mode is executable: all a Snapshot that observed no modes has to give.
    pub(super) executable: bool,
    /// Whether it is a link, its blob the target text (ticket 49).
    pub(super) link: bool,
}

/// What the Snapshot has for one path of a Restore or an Export.
#[derive(Debug, Clone)]
pub(super) enum Version {
    /// A file's content.
    Held(Source),
    /// Nothing, on purpose, and why.
    Withheld(WithheldReason),
}

/// Every temporary name this process hands out is different, without a clock (ADR 0012).
pub(super) static TEMPORARY: AtomicU64 = AtomicU64::new(0);

impl HistoryStore {
    /// The files `scope` covers in its Snapshot, and what is on disk where each would go.
    pub fn restore_preflight(&self, scope: &RestoreScope<'_>) -> Result<Preflight, StoreError> {
        let sources = self.versions(scope.from, scope.paths, false)?;
        let mut found = Vec::with_capacity(sources.len());
        let mut kept = Vec::with_capacity(sources.len());
        for (path, version) in sources {
            let target = scope
                .renamed
                .get(&path)
                .cloned()
                .unwrap_or_else(|| path.clone());
            match version {
                Version::Held(source) => {
                    found.push(facts(scope, &path, &target, &source));
                    kept.push(Some(source));
                }
                // Nothing to write, so nothing on disk to look at.
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
                    kept.push(None);
                }
            }
        }
        fill_links_inside(scope, &mut found);

        Ok(Preflight {
            found,
            sources: kept,
        })
    }

    /// Write what `plan` says, path by path, each re-checked first: a path whose facts changed since the
    /// preflight fails rather than being written over something nobody was asked about.
    ///
    /// Each file is written beside its destination and renamed into place, so no reader ever sees half of
    /// it, and a failure leaves what was there.
    pub fn restore_write(
        &self,
        scope: &RestoreScope<'_>,
        preflight: &Preflight,
        plan: &Plan,
    ) -> Vec<Written> {
        plan.paths
            .iter()
            .zip(&preflight.found)
            .zip(&preflight.sources)
            .map(|((planned, before), source)| match planned.step {
                Step::Unchanged => Written::Unchanged,
                Step::Skipped | Step::Blocked => Written::Skipped,
                Step::Restore | Step::Create => {
                    let Some(source) = source else {
                        return Written::Skipped;
                    };
                    let mut now = facts(scope, &before.path, &before.target, source);
                    fill_links_inside(scope, std::slice::from_mut(&mut now));
                    if !still_as_planned(before, &now) {
                        return Written::Failed(
                            "it changed between the preflight and the write".to_owned(),
                        );
                    }
                    // The content is already the version's: only its mode is restored.
                    let written = match (&now.existing, source.mode) {
                        (Existing::Same, Some(mode)) => set_in_place(scope, &planned.target, mode),
                        _ => self.write_one(scope, planned, &now, source),
                    };
                    match written {
                        Ok(()) if planned.step == Step::Create => Written::Created,
                        Ok(()) => Written::Restored,
                        Err(error) => Written::Failed(error.to_string()),
                    }
                }
            })
            .collect()
    }

    /// Every file of the Snapshot `from` under `paths`, in path order, each once — with every Withheld
    /// path beneath a directory asked for, which the answer names as passed over. No paths is the whole
    /// Snapshot.
    ///
    /// A Withheld path asked for by name is refused, as ticket 10 decided for Restore: there is nothing to
    /// restore. With `named_withheld` it is passed over like any other, as an Export passes it over
    /// (ticket 19).
    pub(super) fn versions(
        &self,
        from: SnapshotId,
        paths: &[RootPath],
        named_withheld: bool,
    ) -> Result<Vec<(RootPath, Version)>, StoreError> {
        let root = self.snapshot_tree(from)?;
        let sidecar = self.sidecar(&root)?.sidecar;
        let modes = sidecar.modes();
        let whole = [RootPath::root()];
        let asked = if paths.is_empty() { &whole[..] } else { paths };

        let mut sources = BTreeMap::new();
        for path in asked {
            let withheld_beneath = |sources: &mut BTreeMap<RootPath, Version>| {
                let mut any = false;
                for record in &sidecar.records {
                    if let Record::Withheld {
                        path: withheld,
                        reason,
                    } = record
                    {
                        if is_beneath(withheld, path) {
                            sources.insert(withheld.clone(), Version::Withheld(*reason));
                            any = true;
                        }
                    }
                }
                any
            };

            if path.is_root() {
                self.collect_sources(&root, &RootPath::root(), &modes, &mut sources)?;
                withheld_beneath(&mut sources);
                continue;
            }
            let entry = match self.entry_at(&root, path) {
                Ok(entry) => entry,
                Err(StoreError::PathNotFound(path)) => {
                    let withheld = withheld_at(&sidecar, &path);
                    // A directory holding only Withheld paths, which git holds no tree for.
                    if withheld.is_none() && withheld_beneath(&mut sources) {
                        continue;
                    }
                    if let (Some(reason), true) = (withheld, named_withheld) {
                        sources.insert(path, Version::Withheld(reason));
                        continue;
                    }
                    return Err(nothing_at(&sidecar, path));
                }
                Err(error) => return Err(error),
            };
            match entry.kind() {
                Some(git2::ObjectType::Tree) => {
                    let tree = self
                        .repository
                        .find_tree(entry.id())
                        .map_err(self.failed("read"))?;
                    self.collect_sources(&tree, path, &modes, &mut sources)?;
                    withheld_beneath(&mut sources);
                }
                Some(git2::ObjectType::Blob) => {
                    let source = self.source(path, &entry, &modes)?;
                    sources.insert(path.clone(), Version::Held(source));
                }
                _ => return Err(StoreError::NotAFile(path.clone())),
            }
        }

        Ok(sources.into_iter().collect())
    }

    fn collect_sources(
        &self,
        tree: &git2::Tree<'_>,
        under: &RootPath,
        modes: &ModeTable,
        sources: &mut BTreeMap<RootPath, Version>,
    ) -> Result<(), StoreError> {
        for entry in tree.iter() {
            let name = String::from_utf8_lossy(entry.name_bytes()).into_owned();
            // The sidecar is the Snapshot's own record, not a file of the Root.
            if under.is_root() && name == keepster_core::SIDECAR {
                continue;
            }
            let path = under.child(&name);
            match entry.kind() {
                Some(git2::ObjectType::Tree) => {
                    let subtree = self
                        .repository
                        .find_tree(entry.id())
                        .map_err(self.failed("read"))?;
                    self.collect_sources(&subtree, &path, modes, sources)?;
                }
                Some(git2::ObjectType::Blob) => {
                    let source = self.source(&path, &entry, modes)?;
                    sources.insert(path, Version::Held(source));
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// The version at `path` — a regular file's, or a link's, which is restored as a link (ticket 49).
    fn source(
        &self,
        path: &RootPath,
        entry: &git2::TreeEntry<'_>,
        modes: &ModeTable,
    ) -> Result<Source, StoreError> {
        const SYMLINK: i32 = 0o120_000;

        let kind = mode_kind(entry.filemode()).unwrap_or(ModeKind::File);
        Ok(Source {
            blob: entry.id(),
            size: self.blob_size(entry.id())?,
            mode: modes.mode_of(path, kind),
            executable: kind == ModeKind::Executable,
            link: entry.filemode() == SYMLINK,
        })
    }

    /// Write one file as the plan says, acting on what is there now — which an earlier file of the same
    /// request may have changed.
    fn write_one(
        &self,
        scope: &RestoreScope<'_>,
        planned: &keepster_core::PathPlan,
        found: &Found,
        source: &Source,
    ) -> io::Result<()> {
        let root = scope.root_path;
        // A file reached through a followed link to it is written where the link leads, which keeps
        // the link a link; a link is always written at its own path.
        let at = absolute(root, &planned.target);
        let through = if source.link {
            None
        } else {
            followed(scope, &planned.target, &at).filter(|real| real.is_file())
        };
        let target = through.clone().unwrap_or(at);
        let parent = target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.to_path_buf());

        match &found.ancestry {
            Ancestry::Present => {}
            Ancestry::Missing => fs::create_dir_all(&parent)?,
            // Confirmed: the file in the way becomes a directory. Its content is in the Safety Snapshot,
            // or the request agreed that it need not be.
            Ancestry::File { at, .. } => {
                fs::remove_file(absolute(root, at))?;
                fs::create_dir_all(&parent)?;
            }
            Ancestry::Link { .. } => {
                return Err(io::Error::other("a directory above it is a link"));
            }
        }

        // A name in another case is the Root's to replace; written through a link, the file the link
        // leads to is replaced, whatever names sit beside the link.
        let replaced = match (&planned.replaces, &through) {
            (Some(replaces), None) => absolute(root, replaces),
            _ => target.clone(),
        };
        let bits = match (planned.modes, found.modes, source.mode) {
            // The bits the user kept are the old file's, but setuid and setgid were granted to the old
            // content, not to this.
            (Some(ModeChoice::Keep), Some(Modes { current, .. }), _) => {
                Bits::Exactly(current & !0o6000)
            }
            (_, _, Some(mode)) => Bits::Exactly(mode),
            // A Snapshot that observed no modes has the executable bit alone to give, applied to the
            // replaced file's bits, or to those the umask gives a new one.
            (_, _, None) => Bits::Executable {
                base: match &found.existing {
                    Existing::File { .. } => fs::metadata(&replaced)
                        .ok()
                        .and_then(|metadata| permissions(&metadata))
                        .map(|mode| mode & 0o777),
                    _ => None,
                },
                executable: source.executable,
            },
        };

        // Short, whatever the file's own name: a long name plus a suffix could exceed what the
        // filesystem allows in one component.
        let temporary = parent.join(format!(
            ".keepster-restore.{}.{}",
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

    /// The version's link at `temporary`, pointing where it pointed.
    pub(super) fn link_temporary(&self, temporary: &Path, source: &Source) -> io::Result<()> {
        let blob = self
            .repository
            .find_blob(source.blob)
            .map_err(|error| io::Error::other(error.to_string()))?;

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;

            std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(blob.content()), temporary)
        }

        // Ticket 76 decides a Windows link's flavour, and refuses one the account cannot create.
        #[cfg(not(unix))]
        {
            let target = String::from_utf8_lossy(blob.content()).replace('/', "\\");
            std::os::windows::fs::symlink_file(target, temporary)
        }
    }

    /// The version's bytes in a new file at `temporary`, synced, with its permission bits set.
    pub(super) fn write_temporary(
        &self,
        temporary: &Path,
        source: &Source,
        bits: Bits,
    ) -> io::Result<()> {
        // A file whose bits are known starts private, so content kept from others is never readable by
        // them for the moment before its bits are set; one whose bits the umask decides starts from it.
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if !matches!(bits, Bits::Executable { base: None, .. }) {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(temporary)?;
        let content = Content {
            store: self,
            blob: source.blob,
            size: source.size,
        };
        content
            .read(0, source.size, &mut |chunk| file.write_all(chunk))
            .map_err(|error| io::Error::other(error.to_string()))?;
        // Through the open file, so the bits land on what was written and on nothing a path could be
        // swapped for in between.
        set_mode(&file, bits)?;

        file.sync_all()
    }
}

/// Whether what is on disk now is what the plan was made for — or what an earlier file of the same
/// request made of it: a directory it created, or put where a file was, and links it broke.
pub(super) fn still_as_planned(before: &Found, now: &Found) -> bool {
    let ancestry = now.ancestry == before.ancestry
        || (matches!(before.ancestry, Ancestry::Missing | Ancestry::File { .. })
            && now.ancestry == Ancestry::Present);
    let existing = match (&before.existing, &now.existing) {
        (
            Existing::File {
                links: was,
                unrecoverable: then,
                ..
            },
            Existing::File {
                links: is,
                unrecoverable: still,
                ..
            },
        ) => then == still && is <= was,
        (before, now) => before == now,
    };

    ancestry && existing && now.case_variant == before.case_variant && now.modes == before.modes
}

/// What is on disk where the file `path` of the Snapshot would go, at `target`.
fn facts(scope: &RestoreScope<'_>, path: &RootPath, target: &RootPath, source: &Source) -> Found {
    let root = scope.root_path;

    let mut ancestry = Ancestry::Present;
    // The last followed link on the way, if the write goes through one.
    let mut via = None;
    for ancestor in target.ancestors() {
        match fs::symlink_metadata(absolute(root, &ancestor)) {
            Err(_) => {
                ancestry = Ancestry::Missing;
                break;
            }
            // A followed link to a directory is written through, as the capture read through it; any
            // other link above the file would lead the write out of what was checked.
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let through = followed(scope, &ancestor, &absolute(root, &ancestor))
                    .is_some_and(|real| real.is_dir());
                if !through {
                    ancestry = Ancestry::Link { at: ancestor };
                    break;
                }
                via = Some(ancestor);
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(metadata) => {
                // A special file in the way — a pipe, a device — is nothing a Snapshot holds.
                let unrecoverable = if metadata.is_file() {
                    recoverability(scope, &ancestor, &metadata).or_else(|| {
                        File::open(absolute(root, &ancestor))
                            .is_err()
                            .then_some(Unrecoverable::Unreadable)
                    })
                } else {
                    Some(Unrecoverable::Unsupported)
                };
                ancestry = Ancestry::File {
                    at: ancestor,
                    unrecoverable,
                };
                break;
            }
        }
    }

    // Through a followed link, the directory the file lands in must not be inside another Root or a
    // History Store, however deep beneath the link's own target it is.
    if let (Ancestry::Present, Some(link)) = (&ancestry, via) {
        let lands = fs::canonicalize(absolute(root, &target.parent()));
        if lands.is_ok_and(|real| scope.policy.stop_following(&real, &[]).is_some()) {
            ancestry = Ancestry::Link { at: link };
        }
    }

    let case_variant = match ancestry {
        Ancestry::Present if scope.naming.folds_case || scope.naming.folds_normalization => {
            variant_of(root, target, scope.naming)
        }
        _ => None,
    };
    let existing = match &ancestry {
        Ancestry::Present => existing(scope, target, source),
        _ => Existing::Absent,
    };
    let modes = match existing {
        Existing::Same | Existing::File { .. } => diverging_modes(scope, target, source),
        _ => None,
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

/// What is at `target`, compared with the version to write.
fn existing(scope: &RestoreScope<'_>, target: &RootPath, source: &Source) -> Existing {
    let at = absolute(scope.root_path, target);
    let Ok(metadata) = fs::symlink_metadata(&at) else {
        return Existing::Absent;
    };

    if metadata.file_type().is_symlink() {
        if source.link {
            let same = fs::read_link(&at).is_ok_and(|text| {
                git2::Oid::hash_object(git2::ObjectType::Blob, &super::link_text(&text))
                    .is_ok_and(|current| current == source.blob)
            });
            if same {
                return Existing::Same;
            }
            // One link replaced by another, in place: no change of kind.
            return Existing::File {
                links: 1,
                links_inside: Vec::new(),
                unrecoverable: recoverability(scope, target, &metadata),
            };
        }
        // A followed link to a file is written through: what is there is the file it leads to.
        if let Some(real) = followed(scope, target, &at) {
            if let Ok(followed) = fs::metadata(&real) {
                if followed.is_file() {
                    return file_existing(scope, target, &real, &followed, source);
                }
            }
        }
        return Existing::Other {
            unrecoverable: recoverability(scope, target, &metadata),
        };
    }
    if metadata.is_dir() {
        return Existing::Directory;
    }
    if !metadata.is_file() {
        return Existing::Other {
            unrecoverable: Some(Unrecoverable::Unsupported),
        };
    }
    if source.link {
        return Existing::Other {
            unrecoverable: unrecoverable_file(scope, target, &at, &metadata),
        };
    }

    file_existing(scope, target, &at, &metadata, source)
}

/// What the regular file at `at` is, compared with the version to write.
fn file_existing(
    scope: &RestoreScope<'_>,
    target: &RootPath,
    at: &Path,
    metadata: &fs::Metadata,
    source: &Source,
) -> Existing {
    // The content alone where the Snapshot observed modes: one that differs is a question of its own
    // (ticket 17). Otherwise the executable bit is part of the version, where the platform keeps it.
    let same_content = git2::Oid::hash_file(git2::ObjectType::Blob, at)
        .is_ok_and(|current| current == source.blob);
    let same_bit = source.mode.is_some()
        || executable(metadata).is_none_or(|current| current == source.executable);
    if same_content && same_bit {
        return Existing::Same;
    }

    Existing::File {
        links: link_count(scope, at, metadata),
        links_inside: Vec::new(),
        unrecoverable: unrecoverable_file(scope, target, at, metadata),
    }
}

/// The permission bits a written file ends up with — none on Windows, which keeps none to set.
#[derive(Debug, Clone, Copy)]
#[cfg_attr(not(unix), allow(dead_code))]
pub(super) enum Bits {
    /// These.
    Exactly(u32),
    /// `base` — the umask's, when `None` — with the executable bit set for every class that may read,
    /// or cleared for all.
    Executable { base: Option<u32>, executable: bool },
}

/// The version's permission bits and those of the regular file at `target` — or the file a followed link
/// there leads to — where the platform keeps them and they differ.
fn diverging_modes(scope: &RestoreScope<'_>, target: &RootPath, source: &Source) -> Option<Modes> {
    let snapshot = source.mode.filter(|_| !source.link)?;
    let at = absolute(scope.root_path, target);
    let at = match fs::symlink_metadata(&at) {
        Ok(metadata) if metadata.file_type().is_symlink() => followed(scope, target, &at)?,
        _ => at,
    };
    let metadata = fs::symlink_metadata(at)
        .ok()
        .filter(fs::Metadata::is_file)?;
    let current = permissions(&metadata)?;

    (current != snapshot).then_some(Modes { snapshot, current })
}

/// Give the file at `target` — or the file a followed link there leads to — the permission bits `mode`,
/// its content already the version's.
fn set_in_place(scope: &RestoreScope<'_>, target: &RootPath, mode: u32) -> io::Result<()> {
    let at = absolute(scope.root_path, target);
    let at = followed(scope, target, &at).unwrap_or(at);
    // Opened without following, so the bits land on the file that was checked and not on whatever a
    // link swapped in for it would lead to.
    let file = super::open_unfollowed(&at)?;

    set_mode(&file, Bits::Exactly(mode))
}

/// Why the regular file at `at` could not be brought back by the Safety Snapshot, if it could not.
fn unrecoverable_file(
    scope: &RestoreScope<'_>,
    target: &RootPath,
    at: &Path,
    metadata: &fs::Metadata,
) -> Option<Unrecoverable> {
    recoverability(scope, target, metadata)
        .or_else(|| File::open(at).is_err().then_some(Unrecoverable::Unreadable))
}

/// Where the link at `path` — `at` on disk — leads, if a Restore may write through it: the follow list
/// still names it, and it leads into no other Root and no History Store (ticket 49).
fn followed(scope: &RestoreScope<'_>, path: &RootPath, at: &Path) -> Option<PathBuf> {
    if !fs::symlink_metadata(at).ok()?.file_type().is_symlink() {
        return None;
    }
    let target = fs::metadata(at).ok()?;
    let kind = if target.is_dir() {
        Kind::Directory
    } else {
        Kind::File
    };
    if !scope.policy.follows(path, kind) {
        return None;
    }
    let real = fs::canonicalize(at).ok()?;

    scope
        .policy
        .stop_following(&real, &[])
        .is_none()
        .then_some(real)
}

/// Why the file at `path` would not enter a Snapshot taken now — the Safety Snapshot — if it would not:
/// the same decision the capture walk makes, ignore files included when the Root honours them.
fn recoverability(
    scope: &RestoreScope<'_>,
    path: &RootPath,
    metadata: &fs::Metadata,
) -> Option<Unrecoverable> {
    recoverability_in(scope.root_path, scope.policy, path, metadata)
}

/// Why the file at `path` in the Root at `root`, under `policy`, would not enter a Snapshot taken now, if
/// it would not.
pub(super) fn recoverability_in(
    root: &Path,
    policy: &CapturePolicy,
    path: &RootPath,
    metadata: &fs::Metadata,
) -> Option<Unrecoverable> {
    let mut policy = policy.clone();
    let mut here = RootPath::root();
    let mut directories: Vec<RootPath> = path.ancestors().collect();
    directories.push(path.clone());

    for next in directories {
        if policy.honours_ignore_files() {
            if let Ok(text) = fs::read_to_string(absolute(root, &here).join(IGNORE_FILE)) {
                let source = here.child(IGNORE_FILE);
                policy = policy.with_ignore_rules(ignore_file_rules(&here, &text, source.as_str()));
            }
        }
        if next == *path {
            break;
        }
        if policy.directory(&next) != Verdict::Captured {
            return Some(Unrecoverable::Excluded);
        }
        here = next;
    }

    match policy.file(path, metadata.len()) {
        Verdict::Captured => None,
        Verdict::TooLarge { .. } => Some(Unrecoverable::TooLarge),
        _ => Some(Unrecoverable::Excluded),
    }
}

/// An entry beside `target` that the destination takes for the same name in another case or spelling.
pub(super) fn variant_of(root: &Path, target: &RootPath, naming: Naming) -> Option<String> {
    let directory = absolute(root, &target.parent());
    let wanted = target.name();

    fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .find(|name| name != wanted && naming.same_name(name, wanted))
}

/// Fill in, for every file with more than one name, its other names inside the Root.
fn fill_links_inside(scope: &RestoreScope<'_>, found: &mut [Found]) {
    for found in found.iter_mut() {
        let target = found.target.clone();
        if let Existing::File {
            links,
            links_inside,
            ..
        } = &mut found.existing
        {
            if *links > 1 {
                *links_inside = other_names(scope, &target);
            }
        }
    }
}

/// The other names inside the Root of the file at `target`.
fn other_names(scope: &RestoreScope<'_>, target: &RootPath) -> Vec<RootPath> {
    let root = scope.root_path;
    let mut names = match scope.link_names {
        Some(list) => list(&absolute(root, target))
            .unwrap_or_default()
            .iter()
            .filter_map(|name| inside_root(root, name))
            .filter(|path| path != target)
            .collect(),
        None => same_inode(root, target),
    };
    names.sort();

    names
}

/// `name` as a path inside `root`, compared as Windows compares names — without case, and whatever
/// prefix either carries — or `None` when it is elsewhere.
fn inside_root(root: &Path, name: &Path) -> Option<RootPath> {
    let plain = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        text.strip_prefix("//?/")
            .map(ToOwned::to_owned)
            .unwrap_or(text)
    };
    let root = plain(root);
    let name = plain(name);
    if name.len() <= root.len() || !name.is_char_boundary(root.len()) {
        return None;
    }
    let (head, rest) = name.split_at(root.len());
    if !head.eq_ignore_ascii_case(&root) {
        return None;
    }

    RootPath::parse(rest.trim_start_matches('/')).ok()
}

/// The other names inside the Root of the file at `target`, found by its `(dev, ino)`.
#[cfg(unix)]
fn same_inode(root: &Path, target: &RootPath) -> Vec<RootPath> {
    use std::os::unix::fs::MetadataExt;

    let Ok(wanted) = fs::metadata(absolute(root, target)) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut directories = vec![RootPath::root()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(absolute(root, &directory)) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let path = directory.child(&name);
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file()
                && metadata.dev() == wanted.dev()
                && metadata.ino() == wanted.ino()
                && path != *target
            {
                names.push(path);
            }
        }
    }

    names
}

#[cfg(not(unix))]
fn same_inode(_: &Path, _: &RootPath) -> Vec<RootPath> {
    Vec::new()
}

/// How many names the file has.
fn link_count(scope: &RestoreScope<'_>, at: &Path, metadata: &fs::Metadata) -> u64 {
    match scope.link_names {
        Some(list) => list(at).map_or(1, |names| names.len().max(1) as u64),
        None => nlink(metadata),
    }
}

#[cfg(unix)]
fn nlink(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    metadata.nlink()
}

#[cfg(not(unix))]
fn nlink(_: &fs::Metadata) -> u64 {
    1
}

/// Whether the file is executable, where the platform says.
#[cfg(unix)]
pub(super) fn executable(metadata: &fs::Metadata) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;

    Some(metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub(super) fn executable(_: &fs::Metadata) -> Option<bool> {
    None
}

/// Give the open file its permission bits: through the file, so they land on what was written and on
/// nothing a path could be swapped for in between.
///
/// The bits are read back, since the system may drop some without an error — setgid for a group the
/// user is not in — and a file that did not get the bits asked for has failed (ticket 17).
#[cfg(unix)]
pub(super) fn set_mode(file: &File, bits: Bits) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mode = match bits {
        Bits::Exactly(mode) => mode,
        Bits::Executable { base, executable } => {
            let base = match base {
                Some(base) => base,
                None => file.metadata()?.permissions().mode() & 0o777,
            };
            if executable {
                base | ((base & 0o444) >> 2)
            } else {
                base & !0o111
            }
        }
    };
    file.set_permissions(fs::Permissions::from_mode(mode))?;

    let set = file.metadata()?.permissions().mode() & 0o7777;
    if set != mode {
        return Err(io::Error::other(format!(
            "its permission bits came out {set:04o}, not {mode:04o}: the system would not set them all"
        )));
    }

    Ok(())
}

/// Windows keeps no permission bits a Snapshot could carry (ticket 50).
#[cfg(not(unix))]
pub(super) fn set_mode(_: &File, _: Bits) -> io::Result<()> {
    Ok(())
}

/// Make a rename in `directory` durable, where the platform lets a directory be synced. Best effort:
/// the file is in place either way, and a failure here is no reason to report it otherwise.
pub(super) fn sync_directory(directory: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = File::open(directory) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = directory;
}

/// Where `path` is on disk beneath `root`.
pub(super) fn absolute(root: &Path, path: &RootPath) -> PathBuf {
    if path.is_root() {
        root.to_path_buf()
    } else {
        root.join(WirePath(path.as_str().to_owned()).as_filesystem_path())
    }
}

/// What the filesystem holding the Root makes of names.
///
/// Whether case is folded is asked of the filesystem itself where an entry of the Root has a name with
/// letters in it — a case-sensitive APFS volume or a case-folding ext4 directory exist, so the platform
/// is only the fallback. Normalisation is folded on macOS, and Windows' naming rules apply on Windows.
pub fn destination_naming(root: &Path) -> Naming {
    let fallback = cfg!(any(target_os = "macos", windows));

    Naming {
        folds_case: folds_case(root).unwrap_or(fallback),
        folds_normalization: cfg!(target_os = "macos"),
        windows_names: cfg!(windows),
    }
}

/// Whether the entry names under `root` are found in another case too, when one tells.
#[cfg(unix)]
fn folds_case(root: &Path) -> Option<bool> {
    use std::os::unix::fs::MetadataExt;

    let entries = fs::read_dir(root).ok()?;
    for entry in entries.filter_map(Result::ok) {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let swapped: String = name
            .chars()
            .map(|character| {
                if character.is_lowercase() {
                    character.to_uppercase().next().unwrap_or(character)
                } else {
                    character.to_lowercase().next().unwrap_or(character)
                }
            })
            .collect();
        if swapped == name {
            continue;
        }
        let Ok(original) = fs::symlink_metadata(root.join(&name)) else {
            continue;
        };
        return Some(match fs::symlink_metadata(root.join(&swapped)) {
            Ok(other) => other.dev() == original.dev() && other.ino() == original.ino(),
            Err(_) => false,
        });
    }

    None
}

#[cfg(not(unix))]
fn folds_case(_: &Path) -> Option<bool> {
    None
}
