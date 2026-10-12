//! Forget (ticket 81, ticket 06): every version of a path removed from a Root's whole history — every
//! Segment and every Pin, the sidecar's records included (ADR 0006) — with every Snapshot keeping its
//! identity, which lives in its message (ADR 0001). The ladder bounds the store to some sixteen hundred
//! commits, so one pass over all of them is affordable, and trees shared between Snapshots are filtered
//! once.
//!
//! What this leaves behind is unreachable, not gone: the caller ends with a Maintenance Pass under the
//! same lock, which is what actually takes the bytes off the disk.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use keepster_core::{Kind, Record, RootPath, Rule, SIDECAR, Sidecar, SnapshotId};

use super::{FORGOTTEN_TRAILER, HistoryStore, OMITTED_TRAILER, PIN_REFS, SEGMENT_REFS, StoreError};
use crate::index;

/// What a Forget did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forgotten {
    /// How many Snapshots held a matching path, and hold none now.
    pub snapshots_touched: u64,
    /// Every path removed, in path order.
    pub paths: Vec<RootPath>,
    /// How many contents no Snapshot holds any more — what the Maintenance Pass that follows removes.
    pub blobs_removed: u64,
    /// The removed paths whose file in the Working Tree still holds a removed version: the next
    /// Snapshot would capture it again, unless the path is excluded from now on.
    pub still_on_disk: Vec<RootPath>,
    /// Refs under the Segments' or the Pins' names that this build could not read, let alone rewrite —
    /// naming no commit, or holding a content format newer than its own (ADR 0010) — so whatever they
    /// hold is still there: reported rather than passed over in silence.
    pub left_alone: Vec<String>,
}

/// One tree with the matching paths taken out.
#[derive(Debug, Clone)]
struct Filtered {
    tree: git2::Oid,
    /// Each path removed, with the content it held — none for a path only the sidecar named.
    removed: Vec<(RootPath, Option<git2::Oid>)>,
    /// How many of the paths removed were recorded as not read, which the Snapshot's count of them
    /// loses.
    incomplete_removed: u64,
}

/// Every source of Snapshots, and the refs among their names this build cannot read.
struct Sources {
    sources: Vec<Source>,
    /// The sources in a content format newer than this build's: never rewritten, but what they hold is
    /// still held.
    opaque: Vec<Source>,
    left_alone: Vec<String>,
}

/// One source of Snapshots: a Segment's chain, or a Pin.
struct Source {
    reference: String,
    tip: git2::Oid,
    /// Its commits, oldest first.
    commits: Vec<git2::Oid>,
    /// A Pin has no parent, whatever came before it.
    chained: bool,
}

impl HistoryStore {
    /// Every path `rule` would remove from this history: what it matches in any Snapshot, held or only
    /// named by the sidecar.
    ///
    /// Only reads: nothing is written to the store, so it needs no lock.
    pub fn forgettable(&self, rule: &Rule) -> Result<BTreeSet<RootPath>, StoreError> {
        let mut filter = Filter::new(self, rule, Writes::None);
        let mut paths = BTreeSet::new();
        for source in self.sources()?.sources {
            for commit in &source.commits {
                let tree = self.tree_of(*commit)?;
                paths.extend(
                    filter
                        .filter(tree, &RootPath::root())?
                        .removed
                        .into_iter()
                        .map(|(path, _)| path),
                );
            }
        }

        Ok(paths)
    }

    /// Remove every version of every path `rule` matches from every Segment and every Pin, saying how
    /// far it got through `progress` — one step a Segment or Pin rewritten, of those to rewrite — and
    /// which files of `working_tree` still hold what was removed.
    ///
    /// A Snapshot left holding nothing is kept, marked like every other it touched: its trailer counts
    /// the paths Forget took from it. Its chronology is the user's, and only they asked for a path to go.
    /// The Working Tree is only read, never written. The caller holds the store's lock.
    pub fn forget(
        &self,
        rule: &Rule,
        working_tree: Option<&Path>,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Forgotten, StoreError> {
        let Sources {
            sources,
            left_alone,
            ..
        } = self.sources()?;
        let mut filter = Filter::new(self, rule, Writes::Trees);
        // Only the sources holding a matching path are rewritten, and only they are steps: a filtered
        // tree is remembered, so looking first costs the rewrite nothing.
        let mut to_rewrite = Vec::new();
        for source in sources {
            let mut holds = false;
            for commit in &source.commits {
                let tree = self.tree_of(*commit)?;
                if filter.filter(tree, &RootPath::root())?.tree != tree {
                    holds = true;
                    break;
                }
            }
            if holds {
                to_rewrite.push(source);
            }
        }
        let total = u64::try_from(to_rewrite.len()).unwrap_or(u64::MAX);
        let mut touched: BTreeSet<SnapshotId> = BTreeSet::new();
        let mut paths: BTreeSet<RootPath> = BTreeSet::new();
        let mut removed_blobs: HashSet<git2::Oid> = HashSet::new();
        let mut held_by: BTreeMap<RootPath, BTreeSet<git2::Oid>> = BTreeMap::new();

        progress(0, total);
        for (done, source) in to_rewrite.iter().enumerate() {
            let mut parent: Option<git2::Oid> = None;
            let mut rewriting = false;
            for commit in &source.commits {
                let original = self
                    .repository
                    .find_commit(*commit)
                    .map_err(self.failed("read"))?;
                let filtered = filter.filter(original.tree_id(), &RootPath::root())?;
                // A tree that changed only by a mode record lost is rewritten too, unmarked: no path
                // went with it.
                let changed = filtered.tree != original.tree_id();
                let marks = !filtered.removed.is_empty();
                if !changed && !rewriting {
                    parent = Some(*commit);
                    continue;
                }

                let mut message = original
                    .message_raw()
                    .map_err(|_| {
                        self.failed("read")(git2::Error::from_str(
                            "a Snapshot's message is not UTF-8, so it cannot be rewritten",
                        ))
                    })?
                    .to_owned();
                let id = original
                    .message()
                    .ok()
                    .and_then(index::trailers)
                    .map(|t| t.id);
                // A commit this product did not write is rewritten with the rest, but its message
                // is not ours to mark.
                if let (true, Some(id)) = (marks, id) {
                    touched.insert(id);
                    let distinct: BTreeSet<&RootPath> =
                        filtered.removed.iter().map(|(path, _)| path).collect();
                    message = marked(
                        &message,
                        u64::try_from(distinct.len()).unwrap_or(u64::MAX),
                        filtered.incomplete_removed,
                    );
                }
                for (path, blob) in &filtered.removed {
                    paths.insert(path.clone());
                    if let Some(blob) = blob {
                        removed_blobs.insert(*blob);
                        held_by.entry(path.clone()).or_default().insert(*blob);
                    }
                }
                let tree = self
                    .repository
                    .find_tree(filtered.tree)
                    .map_err(self.failed("read"))?;
                let parents: Vec<git2::Commit<'_>> = match (source.chained, parent) {
                    (true, Some(parent)) => vec![
                        self.repository
                            .find_commit(parent)
                            .map_err(self.failed("read"))?,
                    ],
                    _ => Vec::new(),
                };
                let parents: Vec<&git2::Commit<'_>> = parents.iter().collect();
                let rewritten = self
                    .repository
                    .commit(
                        None,
                        &original.author(),
                        &original.committer(),
                        &message,
                        &tree,
                        &parents,
                    )
                    .map_err(self.failed("written"))?;
                parent = Some(rewritten);
                rewriting = true;
            }

            if rewriting {
                let new_tip = parent.expect("a rewritten source has a tip");
                self.repository
                    .reference_matching(
                        &source.reference,
                        new_tip,
                        true,
                        source.tip,
                        "keepster: rewritten by Forget",
                    )
                    .map_err(self.failed("written"))?;
            }
            progress(u64::try_from(done + 1).unwrap_or(u64::MAX), total);
        }

        // A content another path still holds is not removed by removing this one.
        let still_held = self.blobs_held()?;
        let blobs_removed = removed_blobs.difference(&still_held).count();
        let still_on_disk = working_tree
            .map(|root| still_on_disk(root, &held_by))
            .unwrap_or_default();
        if total > 0 {
            self.catch_up()?;
        }

        Ok(Forgotten {
            snapshots_touched: u64::try_from(touched.len()).unwrap_or(u64::MAX),
            paths: paths.into_iter().collect(),
            blobs_removed: u64::try_from(blobs_removed).unwrap_or(u64::MAX),
            still_on_disk,
            left_alone,
        })
    }

    /// Every Segment and every Pin, as read now — a Segment holding a commit this product did not write
    /// as well: what a secret is in matters less than that it goes.
    fn sources(&self) -> Result<Sources, StoreError> {
        let mut sources = Vec::new();
        let mut opaque = Vec::new();
        let mut left_alone = Vec::new();
        for (refs, chained) in [(SEGMENT_REFS, true), (PIN_REFS, false)] {
            let references = self
                .repository
                .references_glob(&format!("{refs}/*"))
                .map_err(self.failed("read"))?;
            for reference in references {
                let reference = reference.map_err(self.failed("read"))?;
                let name = String::from_utf8_lossy(reference.name_bytes()).into_owned();
                let Ok(tip) = reference.peel_to_commit().map(|commit| commit.id()) else {
                    left_alone.push(name);
                    continue;
                };
                let source = Source {
                    reference: name,
                    tip,
                    commits: if chained { self.chain(tip)? } else { vec![tip] },
                    chained,
                };
                if self.is_readable(tip)? {
                    sources.push(source);
                } else {
                    left_alone.push(source.reference.clone());
                    opaque.push(source);
                }
            }
        }

        Ok(Sources {
            sources,
            opaque,
            left_alone,
        })
    }

    /// Every content any Segment or Pin still holds.
    fn blobs_held(&self) -> Result<HashSet<git2::Oid>, StoreError> {
        let mut held = HashSet::new();
        let mut seen = HashSet::new();
        let Sources {
            sources, opaque, ..
        } = self.sources()?;
        for source in sources.into_iter().chain(opaque) {
            for commit in source.commits {
                let tree = self.tree_of(commit)?;
                self.collect_blobs(tree, &mut seen, &mut held)?;
            }
        }

        Ok(held)
    }

    fn collect_blobs(
        &self,
        tree: git2::Oid,
        seen: &mut HashSet<git2::Oid>,
        held: &mut HashSet<git2::Oid>,
    ) -> Result<(), StoreError> {
        if !seen.insert(tree) {
            return Ok(());
        }
        let tree = self
            .repository
            .find_tree(tree)
            .map_err(self.failed("read"))?;
        for entry in tree.iter() {
            match entry.kind() {
                Some(git2::ObjectType::Tree) => self.collect_blobs(entry.id(), seen, held)?,
                _ => {
                    held.insert(entry.id());
                }
            }
        }

        Ok(())
    }

    fn tree_of(&self, commit: git2::Oid) -> Result<git2::Oid, StoreError> {
        self.repository
            .find_commit(commit)
            .map(|commit| commit.tree_id())
            .map_err(self.failed("read"))
    }
}

/// Whether a [`Filter`] writes the trees it makes, or only says what it would remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Writes {
    /// Nothing is written: what a look without the store's lock may do.
    None,
    /// Every filtered tree and sidecar is written, under the store's lock.
    Trees,
}

/// Trees with what a rule matches taken out, each filtered once however many Snapshots share it.
struct Filter<'a> {
    store: &'a HistoryStore,
    rule: &'a Rule,
    writes: Writes,
    done: HashMap<(git2::Oid, String), Filtered>,
}

impl<'a> Filter<'a> {
    fn new(store: &'a HistoryStore, rule: &'a Rule, writes: Writes) -> Self {
        Self {
            store,
            rule,
            writes,
            done: HashMap::new(),
        }
    }

    /// The tree `tree`, found at `directory`, without what the rule matches.
    fn filter(&mut self, tree: git2::Oid, directory: &RootPath) -> Result<Filtered, StoreError> {
        // Keyed by where the tree is too: an anchored pattern matches the same tree differently there.
        let key = (tree, directory.as_str().to_owned());
        if let Some(done) = self.done.get(&key) {
            return Ok(done.clone());
        }

        let repository = &self.store.repository;
        let failed = |operation| self.store.failed(operation);
        let original = repository.find_tree(tree).map_err(failed("read"))?;
        let mut builder = repository
            .treebuilder(Some(&original))
            .map_err(failed("read"))?;
        let mut removed = Vec::new();
        let mut incomplete_removed = 0;
        let mut changed = false;

        for entry in original.iter() {
            let name = String::from_utf8_lossy(entry.name_bytes()).into_owned();
            if directory.is_root() && name == SIDECAR {
                continue;
            }
            let path = directory.child(&name);
            match entry.kind() {
                Some(git2::ObjectType::Tree) => {
                    if self.rule.matches(&path, Kind::Directory) {
                        self.everything_beneath(entry.id(), &path, &mut removed)?;
                        builder
                            .remove(entry.name_bytes().to_vec())
                            .map_err(failed("written"))?;
                        changed = true;
                        continue;
                    }
                    let inner = self.filter(entry.id(), &path)?;
                    if inner.tree != entry.id() {
                        changed = true;
                        let empty = repository
                            .find_tree(inner.tree)
                            .map_err(failed("read"))?
                            .is_empty();
                        if empty {
                            builder
                                .remove(entry.name_bytes().to_vec())
                                .map_err(failed("written"))?;
                        } else {
                            builder
                                .insert(entry.name_bytes().to_vec(), inner.tree, entry.filemode())
                                .map_err(failed("written"))?;
                        }
                    }
                    removed.extend(inner.removed);
                    incomplete_removed += inner.incomplete_removed;
                }
                _ => {
                    if self.rule.matches(&path, Kind::File) {
                        removed.push((path, Some(entry.id())));
                        builder
                            .remove(entry.name_bytes().to_vec())
                            .map_err(failed("written"))?;
                        changed = true;
                    }
                }
            }
        }

        if directory.is_root() {
            if let Some(entry) = original.get_name(SIDECAR) {
                let blob = repository.find_blob(entry.id()).map_err(failed("read"))?;
                // A sidecar a later build wrote is left as it is: what it says cannot be rewritten
                // without understanding it, and it holds names, never content.
                if let Ok(parsed) = Sidecar::parse(blob.content()) {
                    let (gone, kept): (Vec<Record>, Vec<Record>) =
                        parsed.sidecar.records.iter().cloned().partition(|record| {
                            // A record is taken as what the tree holds at its path: a pattern for
                            // directories leaves a file's record alone. A path the tree does not hold
                            // — Withheld, or not read — may be either.
                            let kind = original
                                .get_path(Path::new(record.path().as_str()))
                                .ok()
                                .map(|entry| match entry.kind() {
                                    Some(git2::ObjectType::Tree) => Kind::Directory,
                                    _ => Kind::File,
                                });
                            forgets(self.rule, record.path(), kind)
                        });
                    if !gone.is_empty() {
                        let held: BTreeSet<RootPath> =
                            removed.iter().map(|(path, _)| path.clone()).collect();
                        for record in &gone {
                            if matches!(record, Record::Incomplete { .. }) {
                                incomplete_removed += 1;
                            }
                            if !matches!(record, Record::Mode { .. })
                                && !held.contains(record.path())
                            {
                                removed.push((record.path().clone(), None));
                            }
                        }
                        let mut rewritten = Sidecar::with_records(kept);
                        rewritten.header = parsed.sidecar.header;
                        let mut bytes = rewritten.to_bytes();
                        // Lines a later build wrote stay, so that build still finds them.
                        for line in &parsed.unknown {
                            bytes.extend_from_slice(line.as_bytes());
                            bytes.push(b'\n');
                        }
                        if self.writes == Writes::Trees {
                            let written = repository.blob(&bytes).map_err(failed("written"))?;
                            builder
                                .insert(SIDECAR, written, entry.filemode())
                                .map_err(failed("written"))?;
                            changed = true;
                        }
                    }
                }
            }
        }

        let tree = if changed && self.writes == Writes::Trees {
            builder.write().map_err(failed("written"))?
        } else {
            tree
        };
        let filtered = Filtered {
            tree,
            removed,
            incomplete_removed,
        };
        self.done.insert(key, filtered.clone());

        Ok(filtered)
    }

    /// Every file beneath the directory `tree` at `directory`, with its content.
    fn everything_beneath(
        &self,
        tree: git2::Oid,
        directory: &RootPath,
        removed: &mut Vec<(RootPath, Option<git2::Oid>)>,
    ) -> Result<(), StoreError> {
        let tree = self
            .store
            .repository
            .find_tree(tree)
            .map_err(self.store.failed("read"))?;
        for entry in tree.iter() {
            let path = directory.child(&String::from_utf8_lossy(entry.name_bytes()));
            match entry.kind() {
                Some(git2::ObjectType::Tree) => {
                    self.everything_beneath(entry.id(), &path, removed)?
                }
                _ => removed.push((path, Some(entry.id()))),
            }
        }

        Ok(())
    }
}

/// Whether the rule takes `path`, of `kind` where it is known: it matches the path, or a directory above
/// it.
fn forgets(rule: &Rule, path: &RootPath, kind: Option<Kind>) -> bool {
    let itself = match kind {
        Some(kind) => rule.matches(path, kind),
        None => rule.matches(path, Kind::File) || rule.matches(path, Kind::Directory),
    };

    itself
        || path
            .ancestors()
            .any(|ancestor| rule.matches(&ancestor, Kind::Directory))
}

/// `message` marked as touched by Forget: its count of forgotten paths raised by `forgotten`, and its
/// count of paths not read lowered by `incomplete_removed` — every other line as it was.
fn marked(message: &str, forgotten: u64, incomplete_removed: u64) -> String {
    let trailer = |line: &str, key: &str| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(": "))
            .and_then(|value| value.trim().parse::<u64>().ok())
    };
    let mut lines: Vec<String> = Vec::new();
    let mut had_forgotten = false;
    for line in message.lines() {
        if let Some(count) = trailer(line, FORGOTTEN_TRAILER) {
            had_forgotten = true;
            lines.push(format!("{FORGOTTEN_TRAILER}: {}", count + forgotten));
        } else if let Some(count) = trailer(line, OMITTED_TRAILER) {
            lines.push(format!(
                "{OMITTED_TRAILER}: {}",
                count.saturating_sub(incomplete_removed)
            ));
        } else {
            lines.push(line.to_owned());
        }
    }
    if !had_forgotten {
        lines.push(format!("{FORGOTTEN_TRAILER}: {forgotten}"));
    }

    lines.join("\n") + "\n"
}

/// The paths of `held_by` whose file under `root` holds one of the contents removed from it.
fn still_on_disk(root: &Path, held_by: &BTreeMap<RootPath, BTreeSet<git2::Oid>>) -> Vec<RootPath> {
    held_by
        .iter()
        .filter(|(path, contents)| {
            let file = root.join(path.as_str());
            file.is_file()
                && git2::Oid::hash_file(git2::ObjectType::Blob, &file)
                    .is_ok_and(|content| contents.contains(&content))
        })
        .map(|(path, _)| path.clone())
        .collect()
}
