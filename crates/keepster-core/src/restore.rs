//! A Restore's preflight, as a value (ticket 47).
//!
//! Restore only overwrites and creates; it never deletes. Before the first byte is written, every path of
//! the request is checked against what is on disk where it would go, and if any path needs a
//! confirmation, a resolution, or cannot be restored at all, nothing is written and every path says why.
//! What is on disk is gathered by the caller; deciding what it means is here, so it is the same on every
//! platform and can be shown before anything happens (ticket 10).

use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::UnicodeNormalization;

use crate::path::{InvalidPath, RootPath};

/// A structural change a request must agree to before it is made. The blanket force agrees to all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confirmation {
    /// Directories that are not there are created.
    CreateParents,
    /// Something of another kind — a file where a directory is needed, a link or a special file where a
    /// file goes — is replaced.
    ReplaceType,
    /// A file with other names is replaced by a new one, so those names keep the old content.
    BreakHardlinks,
    /// A file is overwritten whose current content the Safety Snapshot cannot hold.
    OverwriteUnrecoverable,
}

impl Confirmation {
    /// Every confirmation, in the order they are reported.
    pub const ALL: [Self; 4] = [
        Self::CreateParents,
        Self::ReplaceType,
        Self::BreakHardlinks,
        Self::OverwriteUnrecoverable,
    ];

    /// The confirmation's wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CreateParents => "create_parents",
            Self::ReplaceType => "replace_type",
            Self::BreakHardlinks => "break_hardlinks",
            Self::OverwriteUnrecoverable => "overwrite_unrecoverable",
        }
    }

    /// The confirmation a wire name names.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.as_str() == name)
    }
}

/// What a request agreed to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Confirmed {
    all: bool,
    these: BTreeSet<Confirmation>,
}

impl Confirmed {
    /// Nothing: every structural change is asked about.
    pub fn none() -> Self {
        Self::default()
    }

    /// The blanket force: every confirmation, and still no resolution.
    pub fn all() -> Self {
        Self {
            all: true,
            these: BTreeSet::new(),
        }
    }

    /// These confirmations.
    pub fn these(confirmations: impl IntoIterator<Item = Confirmation>) -> Self {
        Self {
            all: false,
            these: confirmations.into_iter().collect(),
        }
    }

    /// Whether `confirmation` was given.
    pub fn covers(&self, confirmation: Confirmation) -> bool {
        self.all || self.these.contains(&confirmation)
    }
}

/// Which name a file restored over a name in another case ends up with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameCase {
    /// The name already on disk.
    KeepExisting,
    /// The name in the Snapshot.
    UseSnapshot,
}

/// A user's answer to a question no confirmation settles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Of the Snapshot paths that are one file on the destination, this one is written; the others are
    /// skipped.
    Write,
    /// The file is written under this bare name in the same directory instead.
    RenameTo(String),
    /// Which name a file restored over a name in another case keeps.
    NameCase(NameCase),
}

/// What the destination's filesystem makes of names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Naming {
    /// `Foo` and `foo` are one name.
    pub folds_case: bool,
    /// A composed and a decomposed spelling of one character are one name, as on macOS.
    pub folds_normalization: bool,
    /// Windows' rules apply: reserved device names, no trailing dot or space, none of `<>:"/\|?*`.
    pub windows_names: bool,
}

impl Naming {
    /// Whether the destination takes `a` and `b` for one name.
    pub fn same_name(self, a: &str, b: &str) -> bool {
        fold(a, self) == fold(b, self)
    }
}

/// Why the content now in place could not be brought back from the Safety Snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unrecoverable {
    /// It is over its size cap, so the Safety Snapshot withholds it.
    TooLarge,
    /// The Include Rules leave it out of every Snapshot.
    Excluded,
    /// It cannot be read.
    Unreadable,
    /// It is a kind of file no Snapshot holds yet.
    Unsupported,
}

impl Unrecoverable {
    /// The reason's wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::Excluded => "excluded",
            Self::Unreadable => "unreadable",
            Self::Unsupported => "unsupported",
        }
    }
}

/// What is where a file would be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Existing {
    /// Nothing.
    Absent,
    /// A file holding exactly the Snapshot's version already.
    Same,
    /// A file holding something else.
    File {
        /// How many names the file has, this one included.
        links: u64,
        /// Its other names inside the Root.
        links_inside: Vec<RootPath>,
        /// Why its content could not be brought back, if it could not.
        unrecoverable: Option<Unrecoverable>,
    },
    /// A directory.
    Directory,
    /// A link, or a special file: something no Snapshot holds yet.
    Other,
}

/// What the directories above a file are on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ancestry {
    /// All of them are directories.
    Present,
    /// One of them is not there.
    Missing,
    /// One of them is a file.
    File {
        /// Which.
        at: RootPath,
        /// Why its content could not be brought back, if it could not.
        unrecoverable: Option<Unrecoverable>,
    },
    /// One of them is a link, which a write would follow out of what was checked.
    Link {
        /// Which.
        at: RootPath,
    },
}

/// What the caller found on disk for one file of the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The file's path in the Snapshot.
    pub path: RootPath,
    /// Where it would be written: its path, or the one a rename resolution gave it ([`target_of`]).
    pub target: RootPath,
    /// The directories above the target.
    pub ancestry: Ancestry,
    /// What is at the target.
    pub existing: Existing,
    /// The name of an entry beside the target that the destination takes for the same name, in another
    /// case or spelling — gathered only where the destination folds names.
    pub case_variant: Option<String>,
}

/// Why a path is not simply written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Issue {
    /// A directory above it is not there.
    ParentMissing,
    /// Something of another kind is at `at` — the path itself or a directory above it.
    TypeChange {
        /// Where.
        at: RootPath,
    },
    /// The file has other names, which would keep the old content.
    Hardlinked {
        /// Its other names inside the Root.
        inside: Vec<RootPath>,
        /// How many more it has outside the Root.
        outside: u64,
    },
    /// What would be overwritten cannot enter the Safety Snapshot.
    OverwriteUnrecoverable {
        /// Why.
        why: Unrecoverable,
    },
    /// Other Snapshot paths are this very file on the destination.
    NameCollision {
        /// Which.
        with: Vec<RootPath>,
    },
    /// The destination cannot hold the name — or the new name given for it is already taken there.
    NameInvalid,
    /// An entry by the same name in another case is there.
    NameCase {
        /// Its name.
        existing: String,
    },
    /// A directory is where the file would go, and whatever is inside it may be beyond what the Safety
    /// Snapshot could bring back. Refused.
    TypeConflict,
    /// A directory above it is a link, so the write would leave what was checked. Refused.
    LinkCheckFailed {
        /// Which.
        at: RootPath,
    },
}

impl Issue {
    /// The reason's wire name.
    pub fn code(&self) -> &'static str {
        match self {
            Self::ParentMissing => "parent_missing",
            Self::TypeChange { .. } => "type_change",
            Self::Hardlinked { .. } => "hardlinked",
            Self::OverwriteUnrecoverable { .. } => "overwrite_unrecoverable",
            Self::NameCollision { .. } => "name_collision",
            Self::NameInvalid => "name_invalid",
            Self::NameCase { .. } => "name_case",
            Self::TypeConflict => "type_conflict",
            Self::LinkCheckFailed { .. } => "link_check_failed",
        }
    }

    /// The confirmation that settles it, for the four that one does.
    pub fn confirmation(&self) -> Option<Confirmation> {
        match self {
            Self::ParentMissing => Some(Confirmation::CreateParents),
            Self::TypeChange { .. } => Some(Confirmation::ReplaceType),
            Self::Hardlinked { .. } => Some(Confirmation::BreakHardlinks),
            Self::OverwriteUnrecoverable { .. } => Some(Confirmation::OverwriteUnrecoverable),
            _ => None,
        }
    }

    /// The resolution that settles it, by its wire name, for the three only the user can answer.
    pub fn resolution(&self) -> Option<&'static str> {
        match self {
            Self::NameCollision { .. } => Some("write"),
            Self::NameInvalid => Some("rename_to"),
            Self::NameCase { .. } => Some("name_case"),
            _ => None,
        }
    }
}

/// What happens to one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// A file in place is replaced.
    Restore,
    /// The file is created.
    Create,
    /// The file already holds the version.
    Unchanged,
    /// Passed over, with the issues that are why.
    Skipped,
    /// Stops the whole request until its issues are answered.
    Blocked,
}

/// One path of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathPlan {
    /// The file's path in the Snapshot.
    pub path: RootPath,
    /// Where it is written.
    pub target: RootPath,
    /// The entry it replaces when that has another name than `target` — a name in another case.
    pub replaces: Option<RootPath>,
    /// What happens to it.
    pub step: Step,
    /// What stands in its way — or, for a skipped path, stood.
    pub issues: Vec<Issue>,
}

/// The whole request's plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Every path, in the order asked.
    pub paths: Vec<PathPlan>,
}

impl Plan {
    /// Whether the request can be carried out: no path is blocked.
    pub fn proceeds(&self) -> bool {
        self.paths.iter().all(|path| path.step != Step::Blocked)
    }
}

/// Where the file at `path` is written, given the user's answer about it: elsewhere only for a rename,
/// which must be a bare name in the same directory.
pub fn target_of(
    path: &RootPath,
    resolution: Option<&Resolution>,
) -> Result<RootPath, InvalidPath> {
    match resolution {
        Some(Resolution::RenameTo(name)) => {
            if name.is_empty() || name.contains('/') {
                return Err(InvalidPath::Empty);
            }
            if name == "." || name == ".." {
                return Err(InvalidPath::Relative);
            }
            if name.contains('\0') {
                return Err(InvalidPath::Nul);
            }
            Ok(path.parent().child(name))
        }
        _ => Ok(path.clone()),
    }
}

/// Decide what happens to every path, given what is on disk where each would go.
///
/// A confirmation settles only the issue it names, and the blanket force all four; a name question is
/// settled only by its resolution. With `skip_unresolvable`, a path still in the way is skipped with its
/// reasons rather than stopping the request.
pub fn plan(
    found: Vec<Found>,
    naming: Naming,
    confirmed: &Confirmed,
    resolutions: &BTreeMap<RootPath, Resolution>,
    skip_unresolvable: bool,
) -> Plan {
    let mut paths: Vec<PathPlan> = found
        .into_iter()
        .map(|found| {
            let resolution = resolutions.get(&found.path);
            on_its_own(found, naming, resolution)
        })
        .collect();

    collide(&mut paths, naming, resolutions);

    for path in &mut paths {
        if path.step == Step::Skipped {
            continue;
        }
        path.issues.retain(|issue| {
            issue
                .confirmation()
                .is_none_or(|confirmation| !confirmed.covers(confirmation))
        });
        if !path.issues.is_empty() {
            path.step = if skip_unresolvable {
                Step::Skipped
            } else {
                Step::Blocked
            };
        }
    }

    Plan { paths }
}

/// One path's step and issues from its own facts alone.
fn on_its_own(found: Found, naming: Naming, resolution: Option<&Resolution>) -> PathPlan {
    let Found {
        path,
        mut target,
        ancestry,
        existing,
        case_variant,
    } = found;
    let mut issues = Vec::new();
    let mut replaces = None;

    let renamed = matches!(resolution, Some(Resolution::RenameTo(_)));
    if (naming.windows_names && !valid_on_windows(&target))
        || (renamed && existing != Existing::Absent)
    {
        issues.push(Issue::NameInvalid);
    }
    match ancestry {
        Ancestry::Present => {}
        Ancestry::Missing => issues.push(Issue::ParentMissing),
        Ancestry::File { at, unrecoverable } => {
            issues.push(Issue::TypeChange { at });
            if let Some(why) = unrecoverable {
                issues.push(Issue::OverwriteUnrecoverable { why });
            }
        }
        Ancestry::Link { at } => issues.push(Issue::LinkCheckFailed { at }),
    }

    let step = match existing {
        Existing::Absent => Step::Create,
        Existing::Same => Step::Unchanged,
        Existing::File {
            links,
            links_inside,
            unrecoverable,
        } => {
            if links > 1 {
                let outside = (links - 1).saturating_sub(links_inside.len() as u64);
                issues.push(Issue::Hardlinked {
                    inside: links_inside,
                    outside,
                });
            }
            if let Some(why) = unrecoverable {
                issues.push(Issue::OverwriteUnrecoverable { why });
            }
            Step::Restore
        }
        Existing::Directory => {
            issues.push(Issue::TypeConflict);
            Step::Restore
        }
        Existing::Other => {
            issues.push(Issue::TypeChange { at: target.clone() });
            issues.push(Issue::OverwriteUnrecoverable {
                why: Unrecoverable::Unsupported,
            });
            Step::Restore
        }
    };

    if step != Step::Unchanged {
        if let Some(variant) = case_variant.filter(|variant| variant != target.name()) {
            let variant = target.parent().child(&variant);
            match resolution {
                Some(Resolution::NameCase(NameCase::KeepExisting)) => {
                    target = variant.clone();
                    replaces = Some(variant);
                }
                Some(Resolution::NameCase(NameCase::UseSnapshot)) => replaces = Some(variant),
                _ => issues.push(Issue::NameCase {
                    existing: variant.name().to_owned(),
                }),
            }
        }
    }

    PathPlan {
        path,
        target,
        replaces,
        step,
        issues,
    }
}

/// Find the paths that are one file on the destination, and settle each group by the one the user chose
/// to write — or ask, when none or several were chosen.
///
/// Where the destination folds nothing, only identical targets collide — two files renamed to one name.
fn collide(paths: &mut [PathPlan], naming: Naming, resolutions: &BTreeMap<RootPath, Resolution>) {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, path) in paths.iter().enumerate() {
        groups
            .entry(fold(path.target.as_str(), naming))
            .or_default()
            .push(position);
    }

    for members in groups.into_values().filter(|members| members.len() > 1) {
        let chosen: Vec<usize> = members
            .iter()
            .copied()
            .filter(|position| resolutions.get(&paths[*position].path) == Some(&Resolution::Write))
            .collect();
        let others = |position: usize, among: &[usize]| -> Vec<RootPath> {
            among
                .iter()
                .filter(|other| **other != position)
                .map(|other| paths[*other].path.clone())
                .collect()
        };

        if let [winner] = chosen[..] {
            for position in members
                .iter()
                .copied()
                .filter(|position| *position != winner)
            {
                let issue = Issue::NameCollision {
                    with: vec![paths[winner].path.clone()],
                };
                paths[position].issues = vec![issue];
                paths[position].step = Step::Skipped;
            }
        } else {
            let named: Vec<(usize, Vec<RootPath>)> = members
                .iter()
                .map(|position| (*position, others(*position, &members)))
                .collect();
            for (position, with) in named {
                paths[position].issues.push(Issue::NameCollision { with });
            }
        }
    }
}

/// A name as the destination compares it.
fn fold(name: &str, naming: Naming) -> String {
    let normalised: String = if naming.folds_normalization {
        name.nfc().collect()
    } else {
        name.to_owned()
    };

    if naming.folds_case {
        normalised.to_lowercase()
    } else {
        normalised
    }
}

/// Whether every component of `path` is a name Windows can hold.
fn valid_on_windows(path: &RootPath) -> bool {
    const RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];

    path.as_str().split('/').all(|name| {
        let stem = name
            .split('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let numbered = |prefix: &str| {
            stem.strip_prefix(prefix)
                .is_some_and(|digit| digit.len() == 1 && ("1"..="9").contains(&digit))
        };

        !name.is_empty()
            && !name.ends_with('.')
            && !name.ends_with(' ')
            && !name
                .chars()
                .any(|character| character < ' ' || "<>:\"\\|?*".contains(character))
            && !RESERVED.contains(&stem.as_str())
            && !numbered("com")
            && !numbered("lpt")
    })
}
