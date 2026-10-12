//! The walk behind a Capture Preview (ticket 57, ticket 20): a directory classified against the rules that
//! would govern it, path by path, by the very verdicts a capture walks by — and nothing captured.
//!
//! It opens no History Store and reads no file: every Withheld reason is derivable from a `stat` or a name,
//! and what it cannot see — a file that `stat`s and then fails to read — is what only the capture finds.
//! An excluded directory is never entered, as a capture never enters one, unless its size is asked for.

use std::fs;
use std::ops::ControlFlow;
use std::path::Path;

use keepster_core::{
    CapturePolicy, IGNORE_FILE, Kind, PreviewClass, PreviewEntry, PreviewKind, RootPath, Rule,
    Verdict, WithheldReason, ignore_file_rules,
};

use super::{Place, StoreError, link_text, special_kind};

/// What a preview walk measures beyond what a capture would.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PreviewOptions {
    /// Whether an excluded directory is entered to measure what it holds — a cost capture never pays.
    pub measure_excluded: bool,
}

/// Walk the directory `start` of the Root at `root` under `policy`, handing every path beneath it to
/// `visit` as it is classified, until the walk ends or `visit` says to stop.
///
/// Paths are the Root's own, so a rule anchored at the Root decides beneath `start` as it would in a
/// capture; `policy` is to carry the ignore files of `start`'s ancestors, which this walk does not pass
/// through. A `start` a capture would never enter is one entry, saying why. A directory that cannot be
/// listed at all is an error; anything beneath it that cannot be looked at is an entry, `Unreadable`.
pub fn preview(
    root: &Path,
    start: &RootPath,
    policy: &CapturePolicy,
    options: &PreviewOptions,
    visit: &mut dyn FnMut(PreviewEntry) -> ControlFlow<()>,
) -> Result<(), StoreError> {
    // As a capture does: a path the platform cannot resolve is walked as it is written.
    let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut walk = Walk {
        root,
        options: *options,
        visit,
    };
    let mut place = Place::root(real(root));
    if !start.as_str().is_empty() {
        let directory = walk.filesystem_path(start);
        let metadata = fs::metadata(&directory).map_err(|source| StoreError::WorkingTree {
            path: directory.clone(),
            source,
        })?;
        let real_start = real(&directory);
        // Where an ancestor keeps a capture out, that is the whole answer.
        let (class, rule) = classified(policy.explain(start, Kind::Directory, None).verdict);
        let fenced = policy.fenced_at(&real_start);
        if class != PreviewClass::Captured || fenced.is_some() {
            let class = fenced.map_or(class, PreviewClass::Withheld);
            let _ = (walk.visit)(PreviewEntry {
                path: start.clone(),
                kind: PreviewKind::Directory,
                class,
                bytes: 0,
                rule,
                pruned: class == PreviewClass::Excluded,
            });
            return Ok(());
        }
        place = place.within(real_start, &metadata);
    }
    // Stopped is not failed: the reader went away, and nothing was asked of the walk any more.
    let _ = walk.directory(start, policy, &place, None, false)?;

    Ok(())
}

/// One walk in progress.
struct Walk<'a> {
    root: &'a Path,
    options: PreviewOptions,
    visit: &'a mut dyn FnMut(PreviewEntry) -> ControlFlow<()>,
}

/// Whether the walk goes on.
type Going = Result<ControlFlow<()>, StoreError>;

impl Walk<'_> {
    /// Classify everything in the directory `here`: under `policy`, or — inside an excluded directory
    /// being measured — as excluded by `excluded`, whatever any rule beneath says.
    ///
    /// `announce` names the directory itself first, once it is known to be listable — so a directory that
    /// cannot be listed is one entry, `Unreadable`, and never also one a capture would enter. The
    /// directory a walk starts in is not announced, and not being able to list it is the walk's error.
    fn directory(
        &mut self,
        here: &RootPath,
        policy: &CapturePolicy,
        place: &Place,
        excluded: Option<&Rule>,
        announce: bool,
    ) -> Going {
        let directory = self.filesystem_path(here);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(source) if !announce => {
                return Err(StoreError::WorkingTree {
                    path: directory,
                    source,
                });
            }
            Err(_) => return Ok(self.unreadable(here.clone(), PreviewKind::Directory)),
        };
        if announce {
            let class = match excluded {
                Some(_) => PreviewClass::Excluded,
                None => PreviewClass::Captured,
            };
            let going = self.emit(
                here.clone(),
                PreviewKind::Directory,
                class,
                0,
                excluded.cloned(),
            );
            if going.is_break() {
                return Ok(going);
            }
        }

        let extended;
        let honoured = policy.honours_ignore_files() && excluded.is_none();
        let ignore_file = honoured
            .then(|| fs::read_to_string(directory.join(IGNORE_FILE)))
            .transpose();
        let policy = match ignore_file {
            Ok(Some(text)) => {
                let source = here.child(IGNORE_FILE);
                extended =
                    policy.with_ignore_rules(ignore_file_rules(here, &text, source.as_str()));
                &extended
            }
            _ => policy,
        };

        for entry in entries {
            let Ok(entry) = entry else {
                // The listing broke off: what it did not reach could not be looked at.
                return Ok(self.unreadable(here.clone(), PreviewKind::Directory));
            };
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                let lossy = here.child(&entry.file_name().to_string_lossy());
                let going = self.emit(
                    lossy,
                    PreviewKind::File,
                    PreviewClass::Withheld(WithheldReason::UnrepresentablePath),
                    0,
                    None,
                );
                if going.is_break() {
                    return Ok(going);
                }
                continue;
            };
            let rooted = here.child(&name);
            let going = match entry.metadata() {
                Ok(metadata) => self.entry(
                    &rooted,
                    &directory.join(&name),
                    &metadata,
                    policy,
                    place,
                    excluded,
                )?,
                Err(_) => self.unreadable(rooted, PreviewKind::File),
            };
            if going.is_break() {
                return Ok(going);
            }
        }

        Ok(ControlFlow::Continue(()))
    }

    /// Classify the path `rooted`, at `absolute`, which `metadata` describes without following it.
    fn entry(
        &mut self,
        rooted: &RootPath,
        absolute: &Path,
        metadata: &fs::Metadata,
        policy: &CapturePolicy,
        place: &Place,
        excluded: Option<&Rule>,
    ) -> Going {
        if let Some(rule) = excluded {
            return self.measure(rooted, absolute, metadata, policy, place, rule);
        }
        if metadata.file_type().is_symlink() {
            return self.link(rooted, absolute, policy, place);
        }
        if metadata.is_dir() {
            let real = place.real.join(name_of(rooted));
            return self.subdirectory(rooted, metadata, policy, place, real, false);
        }
        if metadata.is_file() {
            let (class, rule) = classified(policy.file(rooted, metadata.len()));
            return Ok(self.emit(
                rooted.clone(),
                PreviewKind::File,
                class,
                metadata.len(),
                rule,
            ));
        }
        match special_kind(metadata) {
            Some(kind) => {
                let (class, rule) = match classified(policy.file(rooted, 0)) {
                    (PreviewClass::Captured, _) => (
                        PreviewClass::Withheld(WithheldReason::SpecialFile(kind)),
                        None,
                    ),
                    other => other,
                };
                Ok(self.emit(rooted.clone(), PreviewKind::Special, class, 0, rule))
            }
            None => Ok(ControlFlow::Continue(())),
        }
    }

    /// A directory, judged and — if a capture would enter it — entered; `followed` for one reached
    /// through a followed link.
    fn subdirectory(
        &mut self,
        rooted: &RootPath,
        metadata: &fs::Metadata,
        policy: &CapturePolicy,
        place: &Place,
        real: std::path::PathBuf,
        followed: bool,
    ) -> Going {
        let (class, rule) = classified(policy.directory(rooted));
        match class {
            PreviewClass::Captured => {
                if let Some(reason) = policy.fenced_at(&real) {
                    return Ok(self.emit(
                        rooted.clone(),
                        PreviewKind::Directory,
                        PreviewClass::Withheld(reason),
                        0,
                        None,
                    ));
                }
                // A directory already on the way here would never end: a capture names the loop.
                if place.revisits(metadata) {
                    return Ok(self.unreadable(rooted.clone(), PreviewKind::Directory));
                }
                let inside = if followed {
                    place.following(real, metadata)
                } else {
                    place.within(real, metadata)
                };
                self.directory(rooted, policy, &inside, None, true)
            }
            PreviewClass::Excluded if self.options.measure_excluded => {
                self.measured_directory(rooted, metadata, policy, place, real, rule.as_ref())
            }
            PreviewClass::Excluded => Ok(self.emit_pruned(rooted.clone(), rule)),
            other => Ok(self.emit(rooted.clone(), PreviewKind::Directory, other, 0, rule)),
        }
    }

    /// A symbolic link: followed where the rules follow it, as a capture follows it, and otherwise kept
    /// as a link, whose size is its target's text.
    fn link(
        &mut self,
        rooted: &RootPath,
        absolute: &Path,
        policy: &CapturePolicy,
        place: &Place,
    ) -> Going {
        let target = fs::metadata(absolute);
        let follows = match &target {
            Ok(metadata) if metadata.is_dir() => policy.follows(rooted, Kind::Directory),
            Ok(_) => policy.follows(rooted, Kind::File),
            Err(_) => policy.follows(rooted, Kind::Directory) || policy.follows(rooted, Kind::File),
        };
        if follows {
            let followed = target.and_then(|metadata| Ok((metadata, fs::canonicalize(absolute)?)));
            let Ok((metadata, real)) = followed else {
                // A target that cannot be reached is a path the capture would mean to hold and could not.
                return Ok(match classified(policy.file(rooted, 0)) {
                    (PreviewClass::Captured, _) => {
                        self.unreadable(rooted.clone(), PreviewKind::Link)
                    }
                    (class, rule) => self.emit(rooted.clone(), PreviewKind::Link, class, 0, rule),
                });
            };
            let stopped = policy.stop_following(&real, &place.walk()).is_some()
                || (metadata.is_dir() && place.revisits(&metadata));
            if !stopped {
                if metadata.is_dir() {
                    return self.subdirectory(rooted, &metadata, policy, place, real, true);
                }
                if metadata.is_file() {
                    let (class, rule) = classified(policy.file(rooted, metadata.len()));
                    return Ok(self.emit(
                        rooted.clone(),
                        PreviewKind::File,
                        class,
                        metadata.len(),
                        rule,
                    ));
                }
                // Never opened, as a capture never opens one: Withheld, by its kind.
                if let Some(kind) = special_kind(&metadata) {
                    let (class, rule) = match classified(policy.file(rooted, 0)) {
                        (PreviewClass::Captured, _) => (
                            PreviewClass::Withheld(WithheldReason::SpecialFile(kind)),
                            None,
                        ),
                        other => other,
                    };
                    return Ok(self.emit(rooted.clone(), PreviewKind::Special, class, 0, rule));
                }
            }
        }

        match fs::read_link(absolute) {
            Ok(text) => {
                let size = u64::try_from(link_text(&text).len()).unwrap_or(u64::MAX);
                let (class, rule) = classified(policy.file(rooted, size));
                Ok(self.emit(rooted.clone(), PreviewKind::Link, class, size, rule))
            }
            Err(_) => Ok(self.unreadable(rooted.clone(), PreviewKind::Link)),
        }
    }

    /// A path inside an excluded directory being measured: excluded by `rule`, its size counted, and a
    /// directory entered — but never through a link, which the excluded subtree's own capture would not
    /// follow either.
    fn measure(
        &mut self,
        rooted: &RootPath,
        absolute: &Path,
        metadata: &fs::Metadata,
        policy: &CapturePolicy,
        place: &Place,
        rule: &Rule,
    ) -> Going {
        let (kind, bytes) = if metadata.file_type().is_symlink() {
            let size = fs::read_link(absolute)
                .map(|text| u64::try_from(link_text(&text).len()).unwrap_or(u64::MAX))
                .unwrap_or(0);
            (PreviewKind::Link, size)
        } else if metadata.is_dir() {
            let real = place.real.join(name_of(rooted));
            return self.measured_directory(rooted, metadata, policy, place, real, Some(rule));
        } else if metadata.is_file() {
            (PreviewKind::File, metadata.len())
        } else {
            (PreviewKind::Special, 0)
        };

        Ok(self.emit(
            rooted.clone(),
            kind,
            PreviewClass::Excluded,
            bytes,
            Some(rule.clone()),
        ))
    }

    /// An excluded directory being measured: entered, everything in it excluded by `rule` — but never a
    /// directory fenced off, which belongs to another Root or to the history itself, nor one already on
    /// the way here.
    fn measured_directory(
        &mut self,
        rooted: &RootPath,
        metadata: &fs::Metadata,
        policy: &CapturePolicy,
        place: &Place,
        real: std::path::PathBuf,
        rule: Option<&Rule>,
    ) -> Going {
        if let Some(reason) = policy.fenced_at(&real) {
            return Ok(self.emit(
                rooted.clone(),
                PreviewKind::Directory,
                PreviewClass::Withheld(reason),
                0,
                None,
            ));
        }
        if place.revisits(metadata) {
            return Ok(self.unreadable(rooted.clone(), PreviewKind::Directory));
        }
        let inside = place.within(real, metadata);
        self.directory(rooted, policy, &inside, rule, true)
    }

    /// Hand one classified path to the reader, saying whether it is still reading.
    fn emit(
        &mut self,
        path: RootPath,
        kind: PreviewKind,
        class: PreviewClass,
        bytes: u64,
        rule: Option<Rule>,
    ) -> ControlFlow<()> {
        (self.visit)(PreviewEntry {
            path,
            kind,
            class,
            bytes,
            rule,
            pruned: false,
        })
    }

    /// An excluded directory, named and never entered.
    fn emit_pruned(&mut self, path: RootPath, rule: Option<Rule>) -> ControlFlow<()> {
        (self.visit)(PreviewEntry {
            path,
            kind: PreviewKind::Directory,
            class: PreviewClass::Excluded,
            bytes: 0,
            rule,
            pruned: true,
        })
    }

    /// A path that could not be looked at: its `stat`, its listing or its link failed.
    fn unreadable(&mut self, path: RootPath, kind: PreviewKind) -> ControlFlow<()> {
        self.emit(path, kind, PreviewClass::Unreadable, 0, None)
    }

    /// Where `here` is on disk.
    fn filesystem_path(&self, here: &RootPath) -> std::path::PathBuf {
        here.as_str()
            .split('/')
            .filter(|part| !part.is_empty())
            .fold(self.root.to_path_buf(), |path, part| path.join(part))
    }
}

/// The last component of `path`: the name a directory listing gave it.
fn name_of(path: &RootPath) -> &str {
    path.as_str().rsplit('/').next().unwrap_or_default()
}

/// What a verdict means for a preview, and the rule behind it.
fn classified(verdict: Verdict<'_>) -> (PreviewClass, Option<Rule>) {
    match verdict {
        Verdict::Captured => (PreviewClass::Captured, None),
        Verdict::Reserved => (PreviewClass::Withheld(WithheldReason::ReservedName), None),
        Verdict::Fenced(reason) => (PreviewClass::Withheld(reason), None),
        Verdict::TooLarge { rule, .. } => (
            PreviewClass::Withheld(WithheldReason::TooLarge),
            rule.cloned(),
        ),
        Verdict::Excluded(rule) | Verdict::Secret(rule) | Verdict::Pruned { rule, .. } => {
            (PreviewClass::Excluded, Some(rule.clone()))
        }
    }
}
