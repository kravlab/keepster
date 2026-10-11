//! Filesystem events mapped onto paths in a Root (ticket 43).
//!
//! The watcher is a latency optimisation, never the correctness mechanism (ticket 02): every platform
//! loses events, several losses are never announced, and an atomic save arrives with no modify event on
//! the saved path. So what this crate reports is only *that* something under a Root changed, and roughly
//! where; the capture it prompts walks the whole Root regardless, and Reconciliation (ticket 44) is what
//! the coverage promise rests on.
//!
//! Include Rules are applied before watches are installed (ADR 0007): on Linux, where every watch is one
//! directory and the per-user budget is finite, an excluded directory — and the Root's own `.git` — is
//! never watched at all. macOS and Windows watch a whole tree with one handle, so there the same rules
//! filter the events instead.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use keepster_core::{
    CapturePolicy, IGNORE_FILE, Kind, RootPath, Verdict, ignore_file_rules, is_reserved,
};
use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// What the watcher reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Something under the Root changed. The paths are the ones that could be named; the list is empty
    /// when none could — a name is not a path this build can represent — and a change is a change all the
    /// same.
    Changed(Vec<RootPath>),
    /// The platform lost events and said so, or the watch failed: anything may have changed, anywhere.
    /// Ticket 44 answers this with a Reconciliation; until then it is a change like any other.
    Rescan,
}

/// Why a Root could not be watched.
#[derive(Debug, thiserror::Error)]
#[error("{path} could not be watched: {source}")]
pub struct WatchError {
    /// The directory involved.
    pub path: PathBuf,
    /// What the platform said.
    #[source]
    pub source: notify::Error,
}

/// How many watches all Roots together may hold, where the platform spends one per directory: half the
/// per-user inotify budget, read once, so other tools of the same user keep the rest (ticket 30). `None`
/// where one watch covers a whole tree and there is no budget to share.
pub fn default_ceiling() -> Option<usize> {
    if !PER_DIRECTORY {
        return None;
    }
    let budget = fs::read_to_string("/proc/sys/fs/inotify/max_user_watches").ok()?;

    budget.trim().parse::<usize>().ok().map(|budget| budget / 2)
}

/// A Root being watched, until this is dropped.
pub struct Watcher {
    shared: Arc<Mutex<Shared>>,
    worker: Option<JoinHandle<()>>,
}

/// What the event thread and the owner both reach.
struct Shared {
    /// `None` once the owner has let go, which closes the event channel and ends the thread.
    watcher: Option<RecommendedWatcher>,
    /// The directories with a watch of their own.
    watched: BTreeSet<RootPath>,
}

impl Watcher {
    /// Watch the Root at `root` under `policy`, handing every change to `report` on a thread of its own.
    pub fn start(
        root: &Path,
        policy: &CapturePolicy,
        report: impl Fn(WatchEvent) + Send + 'static,
    ) -> Result<Self, WatchError> {
        let (raw, events) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event| {
            // The receiving thread ends only after the watcher is dropped, so a send cannot fail while
            // this closure can run.
            let _ = raw.send(event);
        })
        .map_err(|source| WatchError {
            path: root.to_path_buf(),
            source,
        })?;
        let shared = Arc::new(Mutex::new(Shared {
            watcher: Some(watcher),
            watched: BTreeSet::new(),
        }));

        let context = Context {
            root: resolved(root),
            policy: policy.clone(),
            shared: Arc::clone(&shared),
        };
        if PER_DIRECTORY {
            let policy = context.policy_within(&RootPath::root(), &context.policy);
            context.arm(&RootPath::root(), &policy, &mut Vec::new())?;
        } else {
            context.watch(&RootPath::root(), RecursiveMode::Recursive)?;
        }

        let worker = std::thread::Builder::new()
            .name("keepster-watch".to_owned())
            .spawn(move || {
                for event in events {
                    if let Some(changed) = context.handle(event) {
                        report(changed);
                    }
                }
            })
            .map_err(|error| WatchError {
                path: root.to_path_buf(),
                source: notify::Error::io(error),
            })?;

        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }

    /// The directories holding a watch of their own, from the Root down: every directory the rules admit
    /// on Linux, the Root alone where one watch covers a whole tree.
    pub fn watched(&self) -> Vec<RootPath> {
        lock(&self.shared).watched.iter().cloned().collect()
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        // Dropping the platform watcher drops the sender inside its handler, which ends the event thread.
        let watcher = lock(&self.shared).watcher.take();
        drop(watcher);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Whether each directory gets a watch of its own: inotify watches one directory per watch, and the
/// recursive mode `notify` offers there would install one on every directory, excluded ones included.
const PER_DIRECTORY: bool = cfg!(target_os = "linux");

/// Everything the event thread needs.
struct Context {
    root: PathBuf,
    policy: CapturePolicy,
    shared: Arc<Mutex<Shared>>,
}

impl Context {
    /// Turn one platform event into a change worth reporting, arming any directory it brought in.
    fn handle(&self, event: notify::Result<notify::Event>) -> Option<WatchEvent> {
        let event = match event {
            Ok(event) => event,
            Err(error) => {
                // A failing watch may have missed anything, so it is reported as the platform's own
                // overflow is rather than swallowed.
                tracing::warn!(root = %self.root.display(), %error, "the watch reported an error");
                return Some(WatchEvent::Rescan);
            }
        };
        if event.need_rescan() {
            return Some(WatchEvent::Rescan);
        }

        let mut changed = Vec::new();
        let mut inside = false;
        for path in &event.paths {
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            inside = true;
            let Some(relative) = root_path(relative) else {
                continue;
            };
            let is_directory = fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir());
            let gone = matches!(
                event.kind,
                EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
            );
            if PER_DIRECTORY && gone && !is_directory {
                // Removed or moved away: whatever watch it had died with it.
                self.forget(&relative);
            }
            let parent = parent_of(&relative);
            let policy = self.policy_at(&parent);
            if !self.admits(&policy, &relative, is_directory) {
                continue;
            }

            // A directory that arrived — created, moved in populated, or created again after being
            // removed — is armed and walked, because nothing will ever name what was already inside it.
            let arrived = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_));
            if PER_DIRECTORY && is_directory && arrived && !relative.is_root() {
                self.forget(&relative);
                let policy = self.policy_within(&relative, &policy);
                if let Err(error) = self.arm(&relative, &policy, &mut changed) {
                    tracing::warn!(%error, "a new directory could not be watched");
                }
            }
            changed.push(relative);
        }

        match (changed.is_empty(), inside) {
            (false, _) => Some(WatchEvent::Changed(changed)),
            // Under the Root, but nothing nameable survived: report a change only when no rule removed
            // it, which is when no path could be named at all.
            (true, true) if event.paths.iter().all(|path| self.unnameable(path)) => {
                Some(WatchEvent::Changed(Vec::new()))
            }
            _ => None,
        }
    }

    /// Drop `directory` and everything beneath it from the watched set, once it is gone.
    fn forget(&self, directory: &RootPath) {
        let prefix = format!("{}/", directory.as_str());
        lock(&self.shared).watched.retain(|watched| {
            watched != directory && (directory.is_root() || !watched.as_str().starts_with(&prefix))
        });
    }

    /// The rules in force inside `directory`: the configured ones plus, when ignore files are honoured,
    /// those of the Root and of every directory down to it — what the capture's walk reads on its way.
    fn policy_at(&self, directory: &RootPath) -> CapturePolicy {
        directory
            .ancestors()
            .chain(std::iter::once(directory.clone()))
            .fold(
                self.policy_within(&RootPath::root(), &self.policy),
                |policy, ancestor| {
                    if ancestor.is_root() {
                        policy
                    } else {
                        self.policy_within(&ancestor, &policy)
                    }
                },
            )
    }

    /// `policy` with the ignore file of `directory` added, when ignore files are honoured.
    fn policy_within(&self, directory: &RootPath, policy: &CapturePolicy) -> CapturePolicy {
        if !policy.honours_ignore_files() {
            return policy.clone();
        }
        let file = directory.child(IGNORE_FILE);
        match fs::read_to_string(self.root.join(file.as_str())) {
            Ok(text) => {
                let rules = ignore_file_rules(directory, &text, file.as_str());
                if rules.is_empty() {
                    policy.clone()
                } else {
                    policy.with_ignore_rules(rules)
                }
            }
            Err(_) => policy.clone(),
        }
    }

    fn unnameable(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root)
            .ok()
            .is_some_and(|relative| root_path(relative).is_none())
    }

    /// Watch `directory` and every directory beneath it that `policy` — the rules in force inside it —
    /// admits, adding the files found there to `found`.
    fn arm(
        &self,
        directory: &RootPath,
        policy: &CapturePolicy,
        found: &mut Vec<RootPath>,
    ) -> Result<(), WatchError> {
        {
            let shared = lock(&self.shared);
            if shared.watched.contains(directory) {
                return Ok(());
            }
        }
        self.watch(directory, RecursiveMode::NonRecursive)?;

        let Ok(entries) = fs::read_dir(self.root.join(directory.as_str())) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let child = directory.child(&name);
            let is_directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if !self.admits(policy, &child, is_directory) {
                continue;
            }
            if is_directory {
                // A subdirectory that cannot be watched — the platform's budget is spent, or it vanished
                // — costs latency only: Reconciliation still covers it. It is said, not hidden.
                let within = self.policy_within(&child, policy);
                if let Err(error) = self.arm(&child, &within, found) {
                    tracing::warn!(%error, "a directory could not be watched");
                }
            } else {
                found.push(child);
            }
        }

        Ok(())
    }

    fn watch(&self, directory: &RootPath, mode: RecursiveMode) -> Result<(), WatchError> {
        let path = self.root.join(directory.as_str());
        let mut shared = lock(&self.shared);
        let Some(watcher) = shared.watcher.as_mut() else {
            return Ok(());
        };
        watcher
            .watch(&path, mode)
            .map_err(|source| WatchError { path, source })?;
        shared.watched.insert(directory.clone());

        Ok(())
    }

    /// Whether `policy` lets `path` in: no ancestor excluded or reserved, and the path itself neither —
    /// the question the capture's walk asks of each entry, asked of a path the walk has not reached.
    fn admits(&self, policy: &CapturePolicy, path: &RootPath, is_directory: bool) -> bool {
        if path.is_root() {
            return true;
        }
        let ancestors_admitted = path
            .ancestors()
            .all(|ancestor| matches!(policy.directory(&ancestor), Verdict::Captured));
        let kind = if is_directory {
            Kind::Directory
        } else {
            Kind::File
        };

        ancestors_admitted && !is_reserved(path) && policy.rules().admits(path, kind)
    }
}

/// The Root as the platform reports paths under it. macOS names a change by its resolved path — under
/// `/private/var` for a Root registered under `/var` — so the prefix events are matched against must be
/// the resolved one. Windows reports a path as it was watched, and resolving there would only add a
/// verbatim `\\?\` prefix.
fn resolved(root: &Path) -> PathBuf {
    if cfg!(unix) {
        root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
    } else {
        root.to_path_buf()
    }
}

/// The directory holding `path`, the Root for a path at its top.
fn parent_of(path: &RootPath) -> RootPath {
    path.ancestors().last().unwrap_or_else(RootPath::root)
}

/// `relative` as a path in the Root, if it is one this build can name.
fn root_path(relative: &Path) -> Option<RootPath> {
    let mut components = Vec::new();
    for component in relative.components() {
        components.push(component.as_os_str().to_str()?);
    }

    RootPath::parse(&components.join("/")).ok()
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    // Nothing in `Shared` is left half-updated by a panic: a watch is either recorded or not.
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
