//! The walk behind a Capture Preview (ticket 57, ticket 20): every path classified as a capture would
//! classify it, by `stat` and name alone, without a History Store — an excluded directory never entered
//! unless its size is asked for, and the walk stopped the moment its reader goes away.

use std::collections::BTreeMap;
use std::fs;
use std::ops::ControlFlow;

use keepster_core::{
    CapturePolicy, Level, PreviewClass, PreviewEntry, PreviewKind, RootPath, Rule, RuleOrigin,
    RuleSet, WithheldReason,
};
use keepster_store::{PreviewOptions, preview};

fn rule(text: &str) -> Rule {
    Rule::parse(
        text,
        RootPath::root(),
        RuleOrigin {
            level: Level::Global,
            source: "config.toml".to_owned(),
            line: Some(1),
        },
    )
    .expect("a pattern")
}

/// A tree with something of every class, under a policy excluding `*.log` and `node_modules/` and
/// capping files at 64 bytes.
fn tree() -> (tempfile::TempDir, CapturePolicy) {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let root = directory.path();
    fs::create_dir_all(root.join("src")).expect("a directory");
    fs::create_dir_all(root.join("node_modules/lib")).expect("a directory");
    fs::create_dir_all(root.join(".git")).expect("a directory");
    fs::write(root.join("src/main.rs"), "fn main() {}").expect("a file");
    fs::write(root.join("build.log"), "x".repeat(10)).expect("a file");
    fs::write(root.join("node_modules/lib/index.js"), "y".repeat(30)).expect("a file");
    fs::write(root.join("big.bin"), "z".repeat(100)).expect("a file");
    fs::write(root.join(".env"), "TOKEN=1").expect("a file");
    let policy = CapturePolicy::new(
        RuleSet::new(vec![rule("*.log"), rule("node_modules/")]),
        RuleSet::default(),
        64,
        Vec::new(),
    );

    (directory, policy)
}

fn walk(root: &std::path::Path, policy: &CapturePolicy, measure: bool) -> Vec<PreviewEntry> {
    let mut seen = Vec::new();
    preview(
        root,
        &RootPath::root(),
        policy,
        &PreviewOptions {
            measure_excluded: measure,
        },
        &mut |entry| {
            seen.push(entry);
            ControlFlow::Continue(())
        },
    )
    .expect("a walk");
    seen
}

fn by_path(entries: &[PreviewEntry]) -> BTreeMap<String, &PreviewEntry> {
    entries
        .iter()
        .map(|entry| (entry.path.as_str().to_owned(), entry))
        .collect()
}

#[test]
fn every_path_is_classified_as_a_capture_would_classify_it() {
    let (directory, policy) = tree();

    let entries = walk(directory.path(), &policy, false);
    let found = by_path(&entries);

    let class = |path: &str| found.get(path).map(|entry| entry.class);
    assert_eq!(class("src"), Some(PreviewClass::Captured));
    assert_eq!(class("src/main.rs"), Some(PreviewClass::Captured));
    assert_eq!(found["src/main.rs"].bytes, 12, "its apparent size");
    assert_eq!(class("build.log"), Some(PreviewClass::Excluded));
    assert_eq!(
        found["build.log"]
            .rule
            .as_ref()
            .map(|rule| rule.text.as_str()),
        Some("*.log"),
        "with the rule that decided"
    );
    assert_eq!(
        class("big.bin"),
        Some(PreviewClass::Withheld(WithheldReason::TooLarge))
    );
    assert_eq!(found["big.bin"].bytes, 100);
    assert_eq!(
        class(".git"),
        Some(PreviewClass::Withheld(WithheldReason::ReservedName))
    );
    assert_eq!(
        class(".env"),
        Some(PreviewClass::Excluded),
        "the secret denylist"
    );
    assert!(found[".env"].rule.is_some(), "named by its denylist entry");

    let modules = found["node_modules"];
    assert_eq!(modules.class, PreviewClass::Excluded);
    assert_eq!(modules.kind, PreviewKind::Directory);
    assert!(modules.pruned, "excluded, and never entered");
    assert!(
        !found.keys().any(|path| path.starts_with("node_modules/")),
        "nothing beneath a pruned directory: {:?}",
        found.keys()
    );
}

#[test]
fn an_excluded_directory_is_measured_only_when_asked() {
    let (directory, policy) = tree();

    let entries = walk(directory.path(), &policy, true);
    let found = by_path(&entries);

    let inside = found["node_modules/lib/index.js"];
    assert_eq!(inside.class, PreviewClass::Excluded);
    assert_eq!(inside.bytes, 30);
    assert_eq!(
        inside.rule.as_ref().map(|rule| rule.text.as_str()),
        Some("node_modules/"),
        "excluded by the rule that pruned its ancestor"
    );
    assert!(!found["node_modules"].pruned, "entered, and measured");
}

#[test]
fn the_walk_stops_the_moment_its_reader_does() {
    let (directory, policy) = tree();
    let mut seen = 0;

    preview(
        directory.path(),
        &RootPath::root(),
        &policy,
        &PreviewOptions {
            measure_excluded: true,
        },
        &mut |_| {
            seen += 1;
            ControlFlow::Break(())
        },
    )
    .expect("stopped, not failed");

    assert_eq!(seen, 1);
}

#[test]
fn a_directory_that_is_not_there_cannot_be_previewed() {
    let directory = tempfile::tempdir().expect("a temporary directory");

    assert!(
        preview(
            &directory.path().join("missing"),
            &RootPath::root(),
            &CapturePolicy::default(),
            &PreviewOptions::default(),
            &mut |_| ControlFlow::Continue(()),
        )
        .is_err()
    );
}

#[test]
fn a_subtree_is_previewed_under_the_root_s_own_rules() {
    let (directory, policy) = tree();
    let start = RootPath::parse("node_modules/lib").expect("a path");

    let mut seen = Vec::new();
    preview(
        directory.path(),
        &start,
        &policy,
        &PreviewOptions::default(),
        &mut |entry| {
            seen.push(entry);
            ControlFlow::Continue(())
        },
    )
    .expect("a walk");

    assert_eq!(
        seen.len(),
        1,
        "one answer for a subtree a capture never enters: {seen:?}"
    );
    assert_eq!(seen[0].path, start);
    assert_eq!(seen[0].class, PreviewClass::Excluded);
    assert_eq!(
        seen[0].rule.as_ref().map(|rule| rule.text.as_str()),
        Some("node_modules/"),
        "pruned by its ancestor's rule"
    );

    let src = walk_from(directory.path(), &policy, "src");
    assert_eq!(
        src.iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["src/main.rs"],
        "the Root's own paths, beneath the subtree only"
    );
}

#[test]
fn measuring_what_is_excluded_never_enters_another_root() {
    let (directory, policy) = tree();
    let other = fs::canonicalize(directory.path().join("node_modules/lib")).expect("its real path");
    let policy = policy.with_real_fences(vec![(other, WithheldReason::OtherRoot)]);

    let entries = walk(directory.path(), &policy, true);
    let found = by_path(&entries);

    assert_eq!(
        found["node_modules/lib"].class,
        PreviewClass::Withheld(WithheldReason::OtherRoot)
    );
    assert!(
        !found.contains_key("node_modules/lib/index.js"),
        "never entered"
    );
}

fn walk_from(root: &std::path::Path, policy: &CapturePolicy, start: &str) -> Vec<PreviewEntry> {
    let mut seen = Vec::new();
    preview(
        root,
        &RootPath::parse(start).expect("a path"),
        policy,
        &PreviewOptions::default(),
        &mut |entry| {
            seen.push(entry);
            ControlFlow::Continue(())
        },
    )
    .expect("a walk");
    seen
}
