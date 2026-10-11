//! Restore's two halves that touch the disk (ticket 47): what is where each file of a Snapshot would go,
//! and the writes themselves. What the facts mean is decided in `keepster_core::plan`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use keepster_core::{
    Ancestry, CapturePolicy, Existing, Found, IGNORE_FILE, Naming, Plan, Record, RootPath,
    SnapshotId, Step, Unrecoverable, Verdict, WithheldReason, ignore_file_rules,
};

use super::{Content, HistoryStore, StoreError, WirePath, is_beneath, nothing_at, withheld_at};

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
    sources: Vec<Option<Source>>,
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
struct Source {
    blob: git2::Oid,
    size: u64,
    executable: bool,
}

/// What the Snapshot has for one path of a Restore.
#[derive(Debug, Clone)]
enum Version {
    /// A file's content.
    Held(Source),
    /// Nothing, on purpose, and why.
    Withheld(WithheldReason),
}

/// Every temporary name this process hands out is different, without a clock (ADR 0012).
static TEMPORARY: AtomicU64 = AtomicU64::new(0);

impl HistoryStore {
    /// The files `scope` covers in its Snapshot, and what is on disk where each would go.
    pub fn restore_preflight(&self, scope: &RestoreScope<'_>) -> Result<Preflight, StoreError> {
        let sources = self.restore_sources(scope)?;
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
                    match self.write_one(scope.root_path, planned, &now, source) {
                        Ok(()) if planned.step == Step::Create => Written::Created,
                        Ok(()) => Written::Restored,
                        Err(error) => Written::Failed(error.to_string()),
                    }
                }
            })
            .collect()
    }

    /// Every file of the Snapshot under the paths asked for, in path order, each once — with every
    /// Withheld path beneath a directory asked for, which the answer names as passed over.
    ///
    /// A Withheld path asked for by name is refused, as ticket 10 decided: there is nothing to restore.
    fn restore_sources(
        &self,
        scope: &RestoreScope<'_>,
    ) -> Result<Vec<(RootPath, Version)>, StoreError> {
        let root = self.snapshot_tree(scope.from)?;
        let sidecar = self.sidecar(&root)?.sidecar;
        let whole = [RootPath::root()];
        let asked = if scope.paths.is_empty() {
            &whole[..]
        } else {
            scope.paths
        };

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
                self.collect_sources(&root, &RootPath::root(), &mut sources)?;
                withheld_beneath(&mut sources);
                continue;
            }
            let entry = match self.entry_at(&root, path) {
                Ok(entry) => entry,
                Err(StoreError::PathNotFound(path)) => {
                    // A directory holding only Withheld paths, which git holds no tree for.
                    if withheld_at(&sidecar, &path).is_none() && withheld_beneath(&mut sources) {
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
                    self.collect_sources(&tree, path, &mut sources)?;
                    withheld_beneath(&mut sources);
                }
                Some(git2::ObjectType::Blob) => {
                    if let Some(source) = self.source(&entry)? {
                        sources.insert(path.clone(), Version::Held(source));
                    }
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
                    self.collect_sources(&subtree, &path, sources)?;
                }
                Some(git2::ObjectType::Blob) => {
                    if let Some(source) = self.source(&entry)? {
                        sources.insert(path, Version::Held(source));
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// A regular file's version, or `None` for a link — which ticket 49 restores as a link.
    fn source(&self, entry: &git2::TreeEntry<'_>) -> Result<Option<Source>, StoreError> {
        const EXECUTABLE: i32 = 0o100_755;
        const SYMLINK: i32 = 0o120_000;

        if entry.filemode() == SYMLINK {
            return Ok(None);
        }

        Ok(Some(Source {
            blob: entry.id(),
            size: self.blob_size(entry.id())?,
            executable: entry.filemode() == EXECUTABLE,
        }))
    }

    /// Write one file as the plan says, acting on what is there now — which an earlier file of the same
    /// request may have changed.
    fn write_one(
        &self,
        root: &Path,
        planned: &keepster_core::PathPlan,
        found: &Found,
        source: &Source,
    ) -> io::Result<()> {
        let target = absolute(root, &planned.target);
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

        let replaced = planned
            .replaces
            .as_ref()
            .map_or_else(|| target.clone(), |replaces| absolute(root, replaces));
        let base_mode = match &found.existing {
            Existing::File { .. } => fs::metadata(&replaced)
                .ok()
                .map(|metadata| mode_of(&metadata)),
            _ => None,
        };

        // Short, whatever the file's own name: a long name plus a suffix could exceed what the
        // filesystem allows in one component.
        let temporary = parent.join(format!(
            ".keepster-restore.{}.{}",
            std::process::id(),
            TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        if let Err(error) = self
            .write_temporary(&temporary, source, base_mode)
            .and_then(|()| fs::rename(&temporary, &replaced))
        {
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

    /// The version's bytes in a new file at `temporary`, synced, with its permissions set.
    fn write_temporary(
        &self,
        temporary: &Path,
        source: &Source,
        base_mode: Option<u32>,
    ) -> io::Result<()> {
        // A file replacing one whose bits are known starts private, so content the old file kept from
        // others is never readable by them for the moment before its bits are set; a new file starts
        // from the umask, as it ends.
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if base_mode.is_some() {
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
        set_mode(&file, base_mode, source.executable)?;

        file.sync_all()
    }
}

/// Whether what is on disk now is what the plan was made for — or what an earlier file of the same
/// request made of it: a directory it created, or put where a file was, and links it broke.
fn still_as_planned(before: &Found, now: &Found) -> bool {
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

    ancestry && existing && now.case_variant == before.case_variant
}

/// What is on disk where the file `path` of the Snapshot would go, at `target`.
fn facts(scope: &RestoreScope<'_>, path: &RootPath, target: &RootPath, source: &Source) -> Found {
    let root = scope.root_path;

    let mut ancestry = Ancestry::Present;
    for ancestor in target.ancestors() {
        match fs::symlink_metadata(absolute(root, &ancestor)) {
            Err(_) => {
                ancestry = Ancestry::Missing;
                break;
            }
            Ok(metadata) if metadata.file_type().is_symlink() => {
                ancestry = Ancestry::Link { at: ancestor };
                break;
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

    Found {
        path: path.clone(),
        target: target.clone(),
        ancestry,
        existing,
        case_variant,
        withheld: None,
    }
}

/// What is at `target`, compared with the version to write.
fn existing(scope: &RestoreScope<'_>, target: &RootPath, source: &Source) -> Existing {
    let at = absolute(scope.root_path, target);
    let Ok(metadata) = fs::symlink_metadata(&at) else {
        return Existing::Absent;
    };

    if metadata.is_dir() {
        return Existing::Directory;
    }
    if !metadata.is_file() {
        return Existing::Other;
    }

    let same_content = git2::Oid::hash_file(git2::ObjectType::Blob, &at)
        .is_ok_and(|current| current == source.blob);
    // Where the platform keeps no executable bit, the content alone decides.
    let mode_matches = executable(&metadata).is_none_or(|current| current == source.executable);
    if same_content && mode_matches {
        return Existing::Same;
    }

    let unrecoverable = recoverability(scope, target, &metadata).or_else(|| {
        File::open(&at)
            .is_err()
            .then_some(Unrecoverable::Unreadable)
    });

    Existing::File {
        links: link_count(scope, &at, &metadata),
        links_inside: Vec::new(),
        unrecoverable,
    }
}

/// Why the file at `path` would not enter a Snapshot taken now — the Safety Snapshot — if it would not:
/// the same decision the capture walk makes, ignore files included when the Root honours them.
fn recoverability(
    scope: &RestoreScope<'_>,
    path: &RootPath,
    metadata: &fs::Metadata,
) -> Option<Unrecoverable> {
    let root = scope.root_path;
    let mut policy = scope.policy.clone();
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
fn variant_of(root: &Path, target: &RootPath, naming: Naming) -> Option<String> {
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
fn executable(metadata: &fs::Metadata) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;

    Some(metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_: &fs::Metadata) -> Option<bool> {
    None
}

/// The permission bits of a file.
#[cfg(unix)]
fn mode_of(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    // Never setuid, setgid or sticky: those were granted to the old content, not to this.
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn mode_of(_: &fs::Metadata) -> u32 {
    0
}

/// The replaced file's permission bits — or, for a new file, those the umask gave it — with the
/// Snapshot's executable bit applied: set for every class that may read, or cleared for all.
#[cfg(unix)]
fn set_mode(file: &File, base: Option<u32>, executable: bool) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let base = match base {
        Some(base) => base,
        None => file.metadata()?.permissions().mode() & 0o777,
    };
    let mode = if executable {
        base | ((base & 0o444) >> 2)
    } else {
        base & !0o111
    };

    file.set_permissions(fs::Permissions::from_mode(mode))
}

/// Windows keeps no mode bits a Snapshot could carry (ticket 10).
#[cfg(not(unix))]
fn set_mode(_: &File, _: Option<u32>, _: bool) -> io::Result<()> {
    Ok(())
}

/// Make a rename in `directory` durable, where the platform lets a directory be synced. Best effort:
/// the file is in place either way, and a failure here is no reason to report it otherwise.
fn sync_directory(directory: &Path) {
    #[cfg(unix)]
    if let Ok(handle) = File::open(directory) {
        let _ = handle.sync_all();
    }
    #[cfg(not(unix))]
    let _ = directory;
}

fn absolute(root: &Path, path: &RootPath) -> PathBuf {
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
