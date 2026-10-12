//! A Capture Preview's summary (ticket 57, ticket 20): what a walk classified, grouped by the rule that
//! decided, with the largest entries on both sides — and whether it is past what v1 claims a Root holds.

use keepster_core::{
    Level, PreviewClass, PreviewEntry, PreviewKind, ROOT_CEILING, RootPath, Rule, RuleOrigin,
    Tally, WithheldReason,
};

fn rule(text: &str, line: u32) -> Rule {
    Rule::parse(
        text,
        RootPath::root(),
        RuleOrigin {
            level: Level::Global,
            source: "config.toml".to_owned(),
            line: Some(line),
        },
    )
    .expect("a pattern")
}

fn file(path: &str, class: PreviewClass, bytes: u64, rule: Option<&Rule>) -> PreviewEntry {
    PreviewEntry {
        path: RootPath::parse(path).expect("a path"),
        kind: PreviewKind::File,
        class,
        bytes,
        rule: rule.cloned(),
        pruned: false,
    }
}

fn pruned(path: &str, rule: &Rule) -> PreviewEntry {
    PreviewEntry {
        path: RootPath::parse(path).expect("a path"),
        kind: PreviewKind::Directory,
        class: PreviewClass::Excluded,
        bytes: 0,
        rule: Some(rule.clone()),
        pruned: true,
    }
}

#[test]
fn what_a_walk_found_is_counted_by_class_and_grouped_by_the_rule_that_decided() {
    let logs = rule("*.log", 3);
    let modules = rule("node_modules/", 12);
    let mut tally = Tally::new(2);
    for entry in [
        file("a.txt", PreviewClass::Captured, 10, None),
        file("b.txt", PreviewClass::Captured, 30, None),
        file("c.txt", PreviewClass::Captured, 20, None),
        file("x.log", PreviewClass::Excluded, 500, Some(&logs)),
        file("y.log", PreviewClass::Excluded, 700, Some(&logs)),
        pruned("node_modules", &modules),
        file(
            "big.iso",
            PreviewClass::Withheld(WithheldReason::TooLarge),
            9_000,
            None,
        ),
        file("locked", PreviewClass::Unreadable, 0, None),
    ] {
        tally.add(&entry);
    }

    let summary = tally.finish();

    assert_eq!(
        (summary.captured.files, summary.captured.bytes),
        (3, 60),
        "content to capture"
    );
    assert_eq!((summary.excluded.files, summary.excluded.bytes), (2, 1_200));
    assert_eq!(
        summary.excluded.pruned, 1,
        "a pruned directory is counted, never measured"
    );
    let too_large = summary
        .withheld
        .get(&WithheldReason::TooLarge)
        .expect("by reason");
    assert_eq!((too_large.files, too_large.bytes), (1, 9_000));
    assert_eq!(summary.unreadable, 1);

    let logs_group = summary
        .groups
        .iter()
        .find(|group| group.rule.as_ref().is_some_and(|rule| rule.text == "*.log"))
        .expect("grouped by the rule");
    assert_eq!((logs_group.files, logs_group.bytes), (2, 1_200));
    let modules_group = summary
        .groups
        .iter()
        .find(|group| {
            group
                .rule
                .as_ref()
                .is_some_and(|rule| rule.text == "node_modules/")
        })
        .expect("grouped by the rule");
    assert_eq!(
        modules_group.pruned,
        vec![RootPath::parse("node_modules").expect("a path")],
        "named, and unmeasured"
    );

    let largest = |side: &[(RootPath, u64)]| {
        side.iter()
            .map(|(path, bytes)| (path.as_str().to_owned(), *bytes))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        largest(&summary.largest_captured),
        [("b.txt".to_owned(), 30), ("c.txt".to_owned(), 20)],
        "the largest first, at most as many as asked"
    );
    assert_eq!(
        largest(&summary.largest_left_out),
        [("big.iso".to_owned(), 9_000), ("y.log".to_owned(), 700)],
        "excluded or withheld, whatever left it out"
    );
}

#[test]
fn a_directory_the_capture_would_enter_is_counted_without_bytes() {
    let mut tally = Tally::new(5);
    tally.add(&PreviewEntry {
        path: RootPath::parse("src").expect("a path"),
        kind: PreviewKind::Directory,
        class: PreviewClass::Captured,
        bytes: 0,
        rule: None,
        pruned: false,
    });

    let summary = tally.finish();

    assert_eq!(summary.captured.directories, 1);
    assert_eq!(summary.captured.files, 0);
    assert!(
        summary.largest_captured.is_empty(),
        "a directory is no entry by size"
    );
}

#[test]
fn past_what_v1_claims_a_root_holds_names_each_resource_it_is_past() {
    // Ticket 30: 100,000 included files, 10,000 included directories, 10 GiB of included content.
    let mut tally = Tally::new(1);
    tally.add(&file(
        "huge.bin",
        PreviewClass::Captured,
        11 * 1024 * 1024 * 1024,
        None,
    ));
    let summary = tally.finish();

    assert_eq!(ROOT_CEILING.past(&summary), vec!["bytes"]);
    assert!(
        ROOT_CEILING.past(&Tally::new(1).finish()).is_empty(),
        "an empty directory is well within it"
    );
}
