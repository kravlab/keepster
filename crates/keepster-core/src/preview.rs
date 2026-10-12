//! A Capture Preview's summary (ticket 57, ticket 20): what a walk of a directory classified, against the
//! rules that would govern it, counted and grouped by the rule that decided — the verdict `explain` gives
//! for one path, mapped over a walk and grouped by its own answer.
//!
//! The size is **content to capture**, the files' own lengths. What the History Store would grow by is
//! not here and never estimated: it is unknowable without hashing every file, which is the capture.

use std::collections::BTreeMap;

use crate::path::RootPath;
use crate::rules::Rule;
use crate::sidecar::WithheldReason;

/// How a walk would treat a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PreviewClass {
    /// Its content enters the Snapshot.
    Captured,
    /// A rule, or the secret denylist, leaves it out.
    Excluded,
    /// It is recorded as there, without its content, and why.
    Withheld(WithheldReason),
    /// It could not even be looked at: its `stat` or its listing failed.
    Unreadable,
}

/// What a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, kept as one: its size is its target's text, which is what is stored.
    Link,
    /// A FIFO, a socket or a device.
    Special,
}

/// One path, as a walk classified it.
#[derive(Debug, Clone)]
pub struct PreviewEntry {
    /// Where, relative to the directory previewed.
    pub path: RootPath,
    /// What it is.
    pub kind: PreviewKind,
    /// What a capture would do with it.
    pub class: PreviewClass,
    /// Its apparent size — a file's length, a link's target text — and nothing for a directory, or for
    /// a directory never entered.
    pub bytes: u64,
    /// The rule that decided, where one did.
    pub rule: Option<Rule>,
    /// Whether it is an excluded directory the walk did not enter, as capture never would.
    pub pruned: bool,
}

/// How many files and directories, and how many bytes of files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    /// Files, links and special files.
    pub files: u64,
    /// Directories.
    pub directories: u64,
    /// The files' apparent sizes, summed.
    pub bytes: u64,
    /// Excluded directories not entered, and so not measured.
    pub pruned: u64,
}

impl Totals {
    /// Count `entry`: a directory as one, anything else as one file of its size, a pruned directory as
    /// unmeasured.
    fn add(&mut self, entry: &PreviewEntry) {
        match entry.kind {
            PreviewKind::Directory if entry.pruned => self.pruned += 1,
            PreviewKind::Directory => self.directories += 1,
            _ => {
                self.files += 1;
                self.bytes = self.bytes.saturating_add(entry.bytes);
            }
        }
    }
}

/// Everything one rule — or one reason, or no rule at all — decided.
#[derive(Debug, Clone)]
pub struct PreviewGroup {
    /// What it decided.
    pub class: PreviewClass,
    /// The rule, or `None` where none did: captured by default, Withheld for its reason, unreadable.
    pub rule: Option<Rule>,
    /// How many files it decided, of how many bytes.
    pub files: u64,
    /// How many directories.
    pub directories: u64,
    /// The files' apparent sizes, summed.
    pub bytes: u64,
    /// The directories it pruned, named and unmeasured.
    pub pruned: Vec<RootPath>,
}

/// What a whole walk found.
#[derive(Debug, Clone, Default)]
pub struct PreviewSummary {
    /// Content to capture.
    pub captured: Totals,
    /// What the rules leave out.
    pub excluded: Totals,
    /// What is recorded without its content, by reason.
    pub withheld: BTreeMap<WithheldReason, Totals>,
    /// How many paths could not be looked at.
    pub unreadable: u64,
    /// Every rule that decided something, and what it decided, largest first.
    pub groups: Vec<PreviewGroup>,
    /// The largest files captured, largest first.
    pub largest_captured: Vec<(RootPath, u64)>,
    /// The largest files left out, largest first: excluded ones that were measured, and those withheld
    /// without their content — a file past the size cap is most often the largest of all.
    pub largest_left_out: Vec<(RootPath, u64)>,
}

/// A summary in the making, entry by entry, as a walk streams them.
#[derive(Debug)]
pub struct Tally {
    top: usize,
    summary: PreviewSummary,
    /// Each group by its key: the class and the rule's text and origin.
    groups: BTreeMap<String, PreviewGroup>,
}

impl Tally {
    /// A tally keeping the `top` largest entries on each side.
    pub fn new(top: usize) -> Self {
        Self {
            top,
            summary: PreviewSummary::default(),
            groups: BTreeMap::new(),
        }
    }

    /// Count `entry`.
    pub fn add(&mut self, entry: &PreviewEntry) {
        let summary = &mut self.summary;
        match entry.class {
            PreviewClass::Captured => summary.captured.add(entry),
            PreviewClass::Excluded => summary.excluded.add(entry),
            PreviewClass::Withheld(reason) => {
                summary.withheld.entry(reason).or_default().add(entry)
            }
            PreviewClass::Unreadable => summary.unreadable += 1,
        }
        let sized = entry.kind != PreviewKind::Directory;
        match entry.class {
            PreviewClass::Captured if sized => {
                keep_largest(&mut summary.largest_captured, entry, self.top);
            }
            PreviewClass::Excluded | PreviewClass::Withheld(_) if sized => {
                keep_largest(&mut summary.largest_left_out, entry, self.top);
            }
            _ => {}
        }

        let group = self
            .groups
            .entry(group_key(entry))
            .or_insert_with(|| PreviewGroup {
                class: entry.class,
                rule: entry.rule.clone(),
                files: 0,
                directories: 0,
                bytes: 0,
                pruned: Vec::new(),
            });
        match entry.kind {
            PreviewKind::Directory if entry.pruned => group.pruned.push(entry.path.clone()),
            PreviewKind::Directory => group.directories += 1,
            _ => {
                group.files += 1;
                group.bytes = group.bytes.saturating_add(entry.bytes);
            }
        }
    }

    /// The summary of everything counted.
    pub fn finish(self) -> PreviewSummary {
        let mut summary = self.summary;
        summary.groups = self.groups.into_values().collect();
        // The largest first, so the rule that decided the most is the first one read.
        summary.groups.sort_by(|a, b| {
            (b.bytes, b.files + b.directories, b.pruned.len()).cmp(&(
                a.bytes,
                a.files + a.directories,
                a.pruned.len(),
            ))
        });

        summary
    }
}

/// What tells one group from another: what was decided, and by which rule, where it was written.
fn group_key(entry: &PreviewEntry) -> String {
    let rule = entry.rule.as_ref().map_or_else(String::new, |rule| {
        format!(
            "{}\u{0}{:?}\u{0}{}\u{0}{:?}",
            rule.text, rule.origin.level, rule.origin.source, rule.origin.line
        )
    });

    format!("{:?}\u{0}{rule}", entry.class)
}

/// Keep `entry` among the `top` largest of `side`, largest first, earlier ones first among equals.
fn keep_largest(side: &mut Vec<(RootPath, u64)>, entry: &PreviewEntry, top: usize) {
    if top == 0 {
        return;
    }
    let at = side.partition_point(|(_, bytes)| *bytes >= entry.bytes);
    if at < top {
        side.insert(at, (entry.path.clone(), entry.bytes));
        side.truncate(top);
    }
}

/// What v1 claims a Root holds (ticket 30): one number per resource, because any one can be reached
/// alone. Above it nothing is refused — registering asks for a confirmation instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ceiling {
    /// Included files.
    pub files: u64,
    /// Included directories.
    pub directories: u64,
    /// Included content, in bytes.
    pub bytes: u64,
}

/// The claim: 100,000 files, 10,000 directories, 10 GiB.
pub const ROOT_CEILING: Ceiling = Ceiling {
    files: 100_000,
    directories: 10_000,
    bytes: 10 * 1024 * 1024 * 1024,
};

impl Ceiling {
    /// Which resources `summary`'s captured content is past: `files`, `directories`, `bytes`.
    pub fn past(&self, summary: &PreviewSummary) -> Vec<&'static str> {
        let captured = summary.captured;
        [
            ("files", captured.files > self.files),
            ("directories", captured.directories > self.directories),
            ("bytes", captured.bytes > self.bytes),
        ]
        .into_iter()
        .filter_map(|(name, past)| past.then_some(name))
        .collect()
    }
}
