//! The watcher against a real filesystem (ticket 43). Every test asserts only that an event eventually
//! arrives, never that one did not: platforms lose, delay and coalesce events, and a test that waits for
//! silence proves nothing but its timeout.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use keepster_core::{CapturePolicy, Level, RootPath, Rule, RuleOrigin, RuleSet};
use keepster_watch::{WatchEvent, Watcher};

/// Long enough for a loaded CI runner; the tests finish as soon as the event arrives.
const PATIENCE: Duration = Duration::from_secs(20);

fn excluding(patterns: &[&str]) -> CapturePolicy {
    let rules = patterns
        .iter()
        .map(|pattern| {
            Rule::parse(
                pattern,
                RootPath::root(),
                RuleOrigin {
                    level: Level::Global,
                    source: "test".to_owned(),
                    line: None,
                },
            )
            .expect("a pattern")
        })
        .collect();

    CapturePolicy::new(
        RuleSet::new(rules),
        RuleSet::default(),
        u64::MAX,
        Vec::new(),
    )
}

fn watching(root: &Path, policy: &CapturePolicy) -> (Watcher, mpsc::Receiver<WatchEvent>) {
    let (sender, receiver) = mpsc::channel();
    let watcher = Watcher::start(root, policy, move |event| {
        let _ = sender.send(event);
    })
    .expect("a watcher");

    (watcher, receiver)
}

/// Wait until an event names `path`.
fn eventually_names(receiver: &mpsc::Receiver<WatchEvent>, path: &str) {
    let deadline = std::time::Instant::now() + PATIENCE;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match receiver.recv_timeout(left) {
            Ok(WatchEvent::Changed(paths)) if paths.iter().any(|named| named.as_str() == path) => {
                return;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    panic!("no event named `{path}` within {PATIENCE:?}");
}

#[test]
fn an_edit_eventually_arrives_as_a_change_to_its_path() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = directory.path().canonicalize().expect("the root");
    std::fs::write(root.join("notes.txt"), b"first").expect("a file");
    let (_watcher, events) = watching(&root, &CapturePolicy::default());

    std::fs::write(root.join("notes.txt"), b"second").expect("an edit");

    eventually_names(&events, "notes.txt");
}

#[test]
fn an_atomic_save_arrives_as_a_change_to_the_saved_path() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = directory.path().canonicalize().expect("the root");
    std::fs::write(root.join("notes.txt"), b"first").expect("a file");
    let (_watcher, events) = watching(&root, &CapturePolicy::default());

    std::fs::write(root.join(".notes.txt.swp"), b"second").expect("the temporary file");
    std::fs::rename(root.join(".notes.txt.swp"), root.join("notes.txt")).expect("the rename");

    eventually_names(&events, "notes.txt");
}

#[test]
fn a_file_in_a_directory_created_after_the_watch_began_eventually_arrives() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = directory.path().canonicalize().expect("the root");
    let (_watcher, events) = watching(&root, &CapturePolicy::default());

    std::fs::create_dir_all(root.join("src/new")).expect("new directories");
    std::fs::write(root.join("src/new/lib.rs"), b"// new").expect("a file in them");

    eventually_names(&events, "src/new/lib.rs");
}

#[cfg(target_os = "linux")]
#[test]
fn an_excluded_directory_is_never_watched() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = directory.path().canonicalize().expect("the root");
    std::fs::create_dir_all(root.join("src/deep")).expect("a directory");
    std::fs::create_dir_all(root.join("node_modules/pkg/lib")).expect("an excluded directory");

    let (watcher, _events) = watching(&root, &excluding(&["node_modules/"]));

    let watched: Vec<String> = watcher
        .watched()
        .into_iter()
        .map(|path| path.as_str().to_owned())
        .collect();
    assert_eq!(watched, ["", "src", "src/deep"]);
}
