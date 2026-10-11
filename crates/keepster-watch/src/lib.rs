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

use keepster_core::{CapturePolicy, Kind, RootPath, Verdict, is_reserved};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

/// What the watcher reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Something under the Root changed. The paths are the ones that could be named; the list is empty
    /// when none could — the platform dropped events and asked for a rescan, or a name is not a path this
    /// build can represent — and a change is a change all the same.
    Changed(Vec<RootPath>),
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
            context.arm(&RootPath::root(), &mut Vec::new())?;
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
        // An error from the platform is a watch that may have missed something; Reconciliation is the
        // answer to that, not this thread.
        let event = event.ok()?;
        if event.need_rescan() {
            return Some(WatchEvent::Changed(Vec::new()));
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
            if !self.admits(&relative, is_directory) {
                continue;
            }

            // A directory that arrived — created, or moved in populated — is armed and walked, because
            // nothing will ever name what was already inside it.
            let arrived = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_));
            if PER_DIRECTORY && is_directory && arrived && !relative.is_root() {
                let _ = self.arm(&relative, &mut changed);
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

    fn unnameable(&self, path: &Path) -> bool {
        path.strip_prefix(&self.root)
            .ok()
            .is_some_and(|relative| root_path(relative).is_none())
    }

    /// Watch `directory` and every directory beneath it the rules admit, adding the files found there to
    /// `found`.
    fn arm(&self, directory: &RootPath, found: &mut Vec<RootPath>) -> Result<(), WatchError> {
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
            if !self.admits(&child, is_directory) {
                continue;
            }
            if is_directory {
                // A subdirectory that cannot be watched — the platform's budget is spent, or it vanished
                // — costs latency only: Reconciliation still covers it.
                let _ = self.arm(&child, found);
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

    /// Whether the rules let `path` in: no ancestor excluded or reserved, and the path itself neither.
    fn admits(&self, path: &RootPath, is_directory: bool) -> bool {
        if path.is_root() {
            return true;
        }
        let ancestors_admitted = path
            .ancestors()
            .all(|ancestor| matches!(self.policy.directory(&ancestor), Verdict::Captured));
        let kind = if is_directory {
            Kind::Directory
        } else {
            Kind::File
        };

        ancestors_admitted
            && !is_reserved(path)
            && self
                .policy
                .rules()
                .last_match(path, kind)
                .is_none_or(|rule| rule.negated)
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
