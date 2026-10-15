//! Include Rules as values: the pattern syntax, the layering, "last matching pattern wins", and the one
//! exception to it — an excluded directory is never entered (ADR 0007). No disk anywhere.

use keepster_core::{Kind, Level, RootPath, Rule, RuleOrigin, RuleSet};

fn path(text: &str) -> RootPath {
    RootPath::parse(text).expect("a valid path")
}

/// Rules written at one level, numbered from line 1 as a file would number them.
fn rules(level: Level, patterns: &[&str]) -> Vec<Rule> {
    patterns
        .iter()
        .enumerate()
        .map(|(index, pattern)| {
            Rule::parse(
                pattern,
                RootPath::root(),
                RuleOrigin {
                    level,
                    source: format!("{level:?}"),
                    line: Some(u32::try_from(index + 1).expect("a small line number")),
                },
            )
            .expect("a valid pattern")
        })
        .collect()
}

fn set(patterns: &[&str]) -> RuleSet {
    RuleSet::new(rules(Level::Root, patterns))
}

fn captured(rules: &RuleSet, text: &str, kind: Kind) -> bool {
    rules.classify(&path(text), kind).captured()
}

#[test]
fn everything_is_captured_when_no_rule_matches() {
    let rules = set(&[]);

    assert!(captured(&rules, "src/main.rs", Kind::File));
    assert!(captured(&rules, "src", Kind::Directory));
}

#[test]
fn a_name_without_a_slash_matches_at_any_depth() {
    let rules = set(&["*.log"]);

    assert!(!captured(&rules, "build.log", Kind::File));
    assert!(!captured(&rules, "a/b/c/build.log", Kind::File));
    assert!(captured(&rules, "build.log.txt", Kind::File));
}

#[test]
fn a_pattern_with_a_slash_is_anchored_to_the_root() {
    let rules = set(&["docs/*.pdf", "/target"]);

    assert!(!captured(&rules, "docs/manual.pdf", Kind::File));
    assert!(captured(&rules, "src/docs/manual.pdf", Kind::File));
    assert!(
        captured(&rules, "docs/deep/manual.pdf", Kind::File),
        "`*` stops at `/`"
    );
    assert!(!captured(&rules, "target", Kind::Directory));
    assert!(captured(&rules, "src/target", Kind::Directory));
}

#[test]
fn a_trailing_slash_matches_directories_only() {
    let rules = set(&["cache/"]);

    assert!(!captured(&rules, "cache", Kind::Directory));
    assert!(!captured(&rules, "a/cache", Kind::Directory));
    assert!(captured(&rules, "cache", Kind::File));
}

#[test]
fn double_stars_cross_directories() {
    let rules = set(&["**/fixtures/*.bin", "logs/**", "a/**/z.txt"]);

    assert!(!captured(&rules, "fixtures/x.bin", Kind::File));
    assert!(!captured(&rules, "deep/er/fixtures/x.bin", Kind::File));
    assert!(!captured(&rules, "logs/2026/10/today.txt", Kind::File));
    assert!(
        captured(&rules, "logs", Kind::Directory),
        "`logs/**` excludes contents, not the directory"
    );
    assert!(
        !captured(&rules, "a/z.txt", Kind::File),
        "`/**/` matches zero directories too"
    );
    assert!(!captured(&rules, "a/b/c/z.txt", Kind::File));
}

#[test]
fn question_marks_and_classes_match_one_character() {
    let rules = set(&["file?.txt", "img[0-9].png", "x[!a].md"]);

    assert!(!captured(&rules, "file1.txt", Kind::File));
    assert!(captured(&rules, "file10.txt", Kind::File));
    assert!(!captured(&rules, "img7.png", Kind::File));
    assert!(captured(&rules, "imgA.png", Kind::File));
    assert!(!captured(&rules, "xb.md", Kind::File));
    assert!(captured(&rules, "xa.md", Kind::File));
}

#[test]
fn the_last_matching_pattern_wins_and_a_negation_re_includes() {
    let rules = set(&["*.log", "!keep.log", "keep.log"]);
    assert!(
        !captured(&rules, "keep.log", Kind::File),
        "the later exclusion wins"
    );

    let rules = set(&["*.log", "!keep.log"]);
    let verdict = rules.classify(&path("a/keep.log"), Kind::File);
    assert!(verdict.captured());
    let decisive = verdict.rule.expect("a rule decided it");
    assert_eq!(decisive.text, "!keep.log");
    assert_eq!(decisive.origin.line, Some(2));
}

#[test]
fn a_more_specific_level_outranks_a_less_specific_one_whatever_the_order_given() {
    let mut given = rules(Level::Root, &["!*.log"]);
    given.extend(rules(Level::Global, &["*.log"]));
    let rules = RuleSet::new(given);

    let verdict = rules.classify(&path("debug.log"), Kind::File);

    assert!(
        verdict.captured(),
        "the Root level re-includes what the global level excluded"
    );
    assert_eq!(verdict.rule.expect("a rule").origin.level, Level::Root);
}

#[test]
fn an_excluded_directory_is_never_entered_so_nothing_beneath_it_comes_back() {
    let rules = set(&["node_modules/", "!node_modules/mylib/**"]);

    let verdict = rules.classify(&path("node_modules/mylib/index.js"), Kind::File);

    assert!(!verdict.captured(), "pruning wins over negation (ADR 0007)");
    let (ancestor, pruner) = verdict.pruned_by.expect("the pruning ancestor is named");
    assert_eq!(ancestor.to_str(), Some("node_modules"));
    assert_eq!(pruner.text, "node_modules/");
    assert_eq!(
        verdict
            .rule
            .expect("the path's own rule is still reported")
            .text,
        "!node_modules/mylib/**",
        "explain says what would have re-included it"
    );
}

#[test]
fn excluding_a_directorys_contents_keeps_negation_working() {
    // ADR 0007's escape hatch: the directory itself stays unexcluded, so it is entered.
    let rules = set(&["node_modules/*", "!node_modules/mylib/"]);

    assert!(captured(&rules, "node_modules/mylib/index.js", Kind::File));
    assert!(!captured(&rules, "node_modules/other/index.js", Kind::File));
}

#[test]
fn rules_from_an_ignore_file_apply_only_beneath_its_directory() {
    let nested = Rule::parse(
        "*.tmp",
        path("sub"),
        RuleOrigin {
            level: Level::IgnoreFile,
            source: "sub/.gitignore".to_owned(),
            line: Some(1),
        },
    )
    .expect("a valid pattern");
    let anchored = Rule::parse(
        "/out",
        path("sub"),
        RuleOrigin {
            level: Level::IgnoreFile,
            source: "sub/.gitignore".to_owned(),
            line: Some(2),
        },
    )
    .expect("a valid pattern");
    let rules = RuleSet::new(vec![nested, anchored]);

    assert!(!captured(&rules, "sub/a/x.tmp", Kind::File));
    assert!(
        captured(&rules, "x.tmp", Kind::File),
        "outside the ignore file's directory"
    );
    assert!(
        !captured(&rules, "sub/out", Kind::Directory),
        "anchored to its own directory"
    );
    assert!(captured(&rules, "out", Kind::Directory));
}

#[test]
fn the_users_rules_outrank_an_ignore_file() {
    let ignore = Rule::parse(
        "*.env.local",
        RootPath::root(),
        RuleOrigin {
            level: Level::IgnoreFile,
            source: ".gitignore".to_owned(),
            line: Some(1),
        },
    )
    .expect("a valid pattern");
    let mut given = rules(Level::Global, &["!notes.env.local"]);
    given.push(ignore);

    assert!(captured(
        &RuleSet::new(given),
        "notes.env.local",
        Kind::File
    ));
}

#[test]
fn a_negation_an_excluded_ancestor_makes_unreachable_is_reported() {
    let rules = set(&[
        "node_modules/",
        "!node_modules/mylib/**",
        "!*.keep",
        "build/*",
        "!build/keep/",
    ]);

    let warnings = rules.unreachable_negations();

    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let warning = &warnings[0];
    assert_eq!(warning.negation.text, "!node_modules/mylib/**");
    assert_eq!(warning.ancestor.to_str(), Some("node_modules"));
    assert_eq!(warning.excluded_by.text, "node_modules/");
}

#[test]
fn a_malformed_pattern_is_refused_rather_than_guessed() {
    let origin = RuleOrigin {
        level: Level::Root,
        source: "test".to_owned(),
        line: None,
    };

    assert!(Rule::parse("", RootPath::root(), origin.clone()).is_err());
    assert!(Rule::parse("!", RootPath::root(), origin.clone()).is_err());
    assert!(Rule::parse("[abc", RootPath::root(), origin).is_err());
}

#[test]
fn a_double_star_inside_a_name_is_an_ordinary_star() {
    // git: "Other consecutive asterisks are considered regular asterisks".
    let rules = set(&["a/**.log"]);

    assert!(!captured(&rules, "a/x.log", Kind::File));
    assert!(
        captured(&rules, "a/b/c.log", Kind::File),
        "it does not cross `/`"
    );
}

#[test]
fn a_class_takes_escapes_and_named_classes() {
    let rules = set(&[r"x[\]]y", "v[[:digit:]].txt"]);

    assert!(!captured(&rules, "x]y", Kind::File));
    assert!(!captured(&rules, "v7.txt", Kind::File));
    assert!(captured(&rules, "va.txt", Kind::File));
}

#[test]
fn an_ignore_file_keeps_an_escaped_trailing_space() {
    let rules = RuleSet::new(keepster_core::ignore_file_rules(
        &RootPath::root(),
        "plain   \nkept\\ \n",
        ".gitignore",
    ));

    assert!(
        !captured(&rules, "plain", Kind::File),
        "unescaped trailing spaces are dropped"
    );
    assert!(
        !captured(&rules, "kept ", Kind::File),
        "an escaped one is part of the name"
    );
    assert!(captured(&rules, "kept", Kind::File));
}

#[test]
fn a_path_lists_its_ancestors_from_the_top() {
    let ancestors: Vec<String> = path("a/b/c.txt")
        .ancestors()
        .map(|ancestor| ancestor.to_lossy().into_owned())
        .collect();

    assert_eq!(ancestors, ["a", "a/b"]);
    assert_eq!(path("top").ancestors().count(), 0);
}

#[test]
fn a_name_that_is_not_utf8_is_matched_by_wildcards_as_any_other_name() {
    // Ticket 74: a pattern is text and cannot spell the stray bytes, but a wildcard covers them.
    let rules = set(&["*.bin", "/build/?", "!keep/[!a].txt"]);
    let odd = |bytes: &[u8]| RootPath::parse_bytes(bytes).expect("valid");

    assert!(!rules.classify(&odd(b"src/\xff.bin"), Kind::File).captured());
    assert!(!rules.classify(&odd(b"build/\xff"), Kind::File).captured());
    assert!(rules.classify(&odd(b"src/\xff.rs"), Kind::File).captured());
}
