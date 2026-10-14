//! The watcher conformance report (ticket 70): for each documented class of change, what the operating
//! system's watcher delivered on one machine, as `notify` reports it before anything of Keepster's
//! filters it. It gates nothing: a class nothing was delivered for is a finding, not a failure, since
//! research 02 documents several that no platform reports.
//!
//! Each [`Class`] acts on a watched root and a directory beside it on the same filesystem; what an action
//! does to the tree is deterministic and tested, what is delivered for it is only ever recorded. Writing
//! through `mmap` (research 02, F-L7) is not among the classes: making one needs `unsafe`, which the
//! workspace denies.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::host::Host;

/// How long nothing must be delivered before an observation is over.
const QUIET: Duration = Duration::from_secs(2);

/// The longest an observation lasts, however much is still being delivered.
const LONGEST: Duration = Duration::from_secs(10);

/// How long nothing must be delivered after the watch starts before a class acts, so that nothing its
/// preparation caused is taken for what its action did.
const SETTLED: Duration = Duration::from_millis(300);

/// The longest the wait for [`SETTLED`] lasts, should the watcher keep delivering.
const SETTLING_AT_MOST: Duration = Duration::from_secs(2);

/// What a [`Delivered`] path outside the watched root reads as.
pub const OUTSIDE: &str = "(outside the root)";

/// One documented class of change.
pub struct Class {
    /// Its name, unique among [`CLASSES`].
    pub name: &'static str,
    /// What it does, for a person reading the report.
    pub summary: &'static str,
    /// Where it is documented.
    pub source: &'static str,
    /// Make what the class acts on, before anything watches: `(root, outside)`.
    pub prepare: fn(&Path, &Path) -> io::Result<()>,
    /// The change itself, while the root is watched: `(root, outside)`.
    pub act: fn(&Path, &Path) -> io::Result<()>,
}

/// Every class the report covers, in its order.
pub const CLASSES: &[Class] = &[
    Class {
        name: "edit_in_place",
        summary: "a file truncated and written again, as `>` and editors that save in place do",
        source: "research 02 §1.5 (truncate-in-place save)",
        prepare: |root, _| fs::write(root.join("notes.txt"), "first"),
        act: |root, _| fs::write(root.join("notes.txt"), "second"),
    },
    Class {
        name: "atomic_save",
        summary: "a temporary file written beside the file and renamed over it, as most editors save",
        source: "research 02 §1.5 (atomic save); ticket 02",
        prepare: |root, _| fs::write(root.join("notes.txt"), "first"),
        act: |root, _| {
            let temporary = root.join(".notes.txt.swp");
            fs::write(&temporary, "second")?;
            fs::rename(temporary, root.join("notes.txt"))
        },
    },
    Class {
        name: "create_file",
        summary: "a new file",
        source: "ticket 02",
        prepare: |_, _| Ok(()),
        act: |root, _| fs::write(root.join("new.txt"), "new"),
    },
    Class {
        name: "delete_file",
        summary: "a file removed",
        source: "ticket 02",
        prepare: |root, _| fs::write(root.join("old.txt"), "old"),
        act: |root, _| fs::remove_file(root.join("old.txt")),
    },
    Class {
        name: "rename_within",
        summary: "a file renamed inside the root",
        source: "research 02, F-L4; ticket 02",
        prepare: |root, _| fs::write(root.join("before.txt"), "renamed"),
        act: |root, _| fs::rename(root.join("before.txt"), root.join("after.txt")),
    },
    Class {
        name: "move_out",
        summary: "a file moved out of the root, which reads like a delete",
        source: "research 02 §1.5 (move out of the tree)",
        prepare: |root, _| fs::write(root.join("leaving.txt"), "leaving"),
        act: |root, outside| fs::rename(root.join("leaving.txt"), outside.join("leaving.txt")),
    },
    Class {
        name: "new_directory_with_a_file",
        summary: "a directory made and a file written into it at once",
        source: "research 02, F-L3",
        prepare: |root, _| fs::create_dir(root.join("src")),
        act: |root, _| {
            let new = root.join("src").join("new");
            fs::create_dir(&new)?;
            fs::write(new.join("lib.rs"), "// new")
        },
    },
    Class {
        name: "populated_directory_moved_in",
        summary: "a directory that already holds files moved into the root",
        source: "research 02, F-L3",
        prepare: |_, outside| {
            let incoming = outside.join("incoming");
            fs::create_dir_all(incoming.join("deep"))?;
            fs::write(incoming.join("a.txt"), "a")?;
            fs::write(incoming.join("deep").join("b.txt"), "b")
        },
        act: |root, outside| fs::rename(outside.join("incoming"), root.join("incoming")),
    },
    Class {
        name: "write_through_a_second_hardlink",
        summary: "a file in the root changed through a hard link to it outside the root",
        source: "research 02, F-L5",
        prepare: |root, outside| {
            fs::write(outside.join("alias.txt"), "first")?;
            fs::hard_link(outside.join("alias.txt"), root.join("linked.txt"))
        },
        act: |_, outside| fs::write(outside.join("alias.txt"), "second"),
    },
    Class {
        name: "permission_only",
        summary: "a file made read-only and nothing else: its mode on Unix, its attribute on Windows",
        source: "ticket 17; research 02, F-M6",
        prepare: |root, _| fs::write(root.join("script.sh"), "echo"),
        act: |root, _| {
            let path = root.join("script.sh");
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_readonly(true);
            fs::set_permissions(path, permissions)
        },
    },
    Class {
        name: "directory_removed_and_created_again",
        summary: "a directory removed with what it held and made again empty, as a clean build does",
        source: "research 02, F-L3; ticket 02",
        prepare: |root, _| {
            fs::create_dir(root.join("build"))?;
            fs::write(root.join("build").join("out.o"), "object")
        },
        act: |root, _| {
            fs::remove_dir_all(root.join("build"))?;
            fs::create_dir(root.join("build"))
        },
    },
];

/// What was delivered for one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    /// Every event, in the order it arrived.
    pub delivered: Vec<Delivered>,
}

/// One event, as `notify` delivered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivered {
    /// Its kind as `notify` spells it, such as `Modify(Data(Any))`; for an error the watcher delivered
    /// instead of an event, `error: ` and the error.
    pub kind: String,
    /// Its paths, relative to the root with `/` between components, or [`OUTSIDE`].
    pub paths: Vec<String>,
    /// Whether it says that events were lost and the tree must be scanned again.
    pub rescan: bool,
}

/// What the watcher of this platform delivers for `class`, acted out in two fresh directories made in
/// `directory`: the root it watches, recursively, and one beside it. The watch starts once the class is
/// prepared; the observation ends after [`QUIET`] with nothing delivered, or at [`LONGEST`].
///
/// # Errors
///
/// When the directories cannot be made, the watch cannot start, or the class cannot be prepared or
/// acted out.
pub fn observe(class: &Class, directory: &Path) -> io::Result<Observation> {
    let scratch = tempfile::tempdir_in(directory)?;
    // As the operating system names it, so that the paths it delivers start with it: on macOS the
    // temporary directory is behind a link.
    let base = scratch.path().canonicalize()?;
    let root = base.join("root");
    let outside = base.join("outside");
    fs::create_dir(&root)?;
    fs::create_dir(&outside)?;
    (class.prepare)(&root, &outside)?;

    let (sender, events) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(sender, notify::Config::default())
        .map_err(|error| io::Error::other(format!("the watcher cannot start: {error}")))?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|error| {
            io::Error::other(format!("{} cannot be watched: {error}", root.display()))
        })?;
    drain(&events, SETTLED, SETTLING_AT_MOST);

    (class.act)(&root, &outside)?;
    let delivered = drain(&events, QUIET, LONGEST)
        .into_iter()
        .map(|event| delivered(&root, event))
        .collect();

    Ok(Observation { delivered })
}

/// Everything `events` delivers until `quiet` passes with nothing, or `longest` has passed.
fn drain<T>(events: &Receiver<T>, quiet: Duration, longest: Duration) -> Vec<T> {
    let end = Instant::now() + longest;
    let mut received = Vec::new();
    loop {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return received;
        }
        match events.recv_timeout(quiet.min(left)) {
            Ok(event) => received.push(event),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return received,
        }
    }
}

/// `event` as the report records it.
fn delivered(root: &Path, event: notify::Result<notify::Event>) -> Delivered {
    match event {
        Ok(event) => Delivered {
            kind: format!("{:?}", event.kind),
            rescan: event.need_rescan(),
            paths: event
                .paths
                .iter()
                .map(|path| relative(root, path))
                .collect(),
        },
        Err(error) => Delivered {
            kind: format!("error: {error}"),
            rescan: false,
            paths: error
                .paths
                .iter()
                .map(|path| relative(root, path))
                .collect(),
        },
    }
}

/// `path` relative to `root`, with `/` between its components whatever the platform, or [`OUTSIDE`].
fn relative(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(inside) => inside
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => OUTSIDE.to_owned(),
    }
}

/// The conformance report of `observations`, made on `host`, in Markdown: a section per class with what
/// it is, where it is documented, and every event delivered for it, a rescan marked. A class whose
/// observation is an error — the machine could not prepare or act it out, or watch it — says so in its
/// section: that is a finding about the machine too, and the report gates nothing.
pub fn render(host: &Host, observations: &[(&Class, Result<Observation, String>)]) -> String {
    let mut text = format!(
        "# Watcher conformance\n\n\
         What this platform's watcher delivered for each documented class of change, as `notify` reports \
         it. It gates nothing: a class nothing was delivered for is a finding, not a failure. Writing \
         through `mmap` is not among the classes: making one needs `unsafe`, which the workspace denies.\n\n\
         Machine: {host}\n"
    );
    for (class, observation) in observations {
        text.push_str(&format!(
            "\n## {}\n\n{}. Source: {}.\n\n",
            class.name, class.summary, class.source
        ));
        let observation = match observation {
            Ok(observation) => observation,
            Err(why) => {
                text.push_str(&format!("- could not be acted out: {why}\n"));
                continue;
            }
        };
        if observation.delivered.is_empty() {
            text.push_str("- nothing was delivered\n");
        }
        for event in &observation.delivered {
            text.push_str(&format!("- `{}`", event.kind));
            let paths: Vec<String> = event
                .paths
                .iter()
                .map(|path| {
                    if path.is_empty() {
                        "the root".to_owned()
                    } else {
                        format!("`{path}`")
                    }
                })
                .collect();
            if !paths.is_empty() {
                text.push_str(&format!(" {}", paths.join(", ")));
            }
            if event.rescan {
                text.push_str(" — rescan");
            }
            text.push('\n');
        }
    }
    text
}
