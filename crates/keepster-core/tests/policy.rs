//! What a capture does with one path, as values: the user's rules, the built-in secret denylist and
//! its opt-in list, the size cap and its per-pattern overrides, and the names no rule may admit.

use keepster_core::{
    CapturePolicy, DEFAULT_MAX_SIZE, Kind, Level, RootPath, Rule, RuleOrigin, RuleSet, SizeRule,
    Verdict, parse_size,
};

const MIB: u64 = 1024 * 1024;

fn path(text: &str) -> RootPath {
    RootPath::parse(text).expect("a valid path")
}

fn rule(text: &str, level: Level) -> Rule {
    Rule::parse(
        text,
        RootPath::root(),
        RuleOrigin {
            level,
            source: "test".to_owned(),
            line: Some(1),
        },
    )
    .expect("a valid pattern")
}

fn default_policy() -> CapturePolicy {
    CapturePolicy::new(
        RuleSet::default(),
        RuleSet::default(),
        DEFAULT_MAX_SIZE,
        Vec::new(),
    )
}

#[test]
fn sizes_are_read_in_binary_and_decimal_units() {
    assert_eq!(parse_size("100MiB"), Ok(100 * MIB));
    assert_eq!(parse_size("1 GiB"), Ok(1024 * MIB));
    assert_eq!(parse_size("500KB"), Ok(500_000));
    assert_eq!(parse_size("2MB"), Ok(2_000_000));
    assert_eq!(parse_size("42"), Ok(42));
    assert_eq!(parse_size("7B"), Ok(7));
    for refused in ["", "ten", "-1MiB", "5XB", "1.5GiB", "MiB"] {
        assert!(parse_size(refused).is_err(), "{refused}");
    }
}

#[test]
fn the_default_cap_is_one_hundred_mebibytes() {
    assert_eq!(DEFAULT_MAX_SIZE, 100 * MIB);
}

#[test]
fn an_ordinary_file_under_the_cap_is_captured() {
    assert_eq!(
        default_policy().file(&path("src/main.rs"), 1_000),
        Verdict::Captured
    );
}

#[test]
fn the_documented_secret_names_are_refused_by_default() {
    let policy = default_policy();

    for secret in [
        ".env",
        "app/.env.production",
        ".envrc",
        "certs/server.pem",
        "tls/private.key",
        "store.p12",
        "store.pfx",
        "release.jks",
        "upload.keystore",
        "home/.ssh/id_rsa",
        "id_dsa",
        "id_ecdsa",
        "id_ed25519",
        ".netrc",
        ".pgpass",
        "android/local.properties",
    ] {
        assert!(
            matches!(policy.file(&path(secret), 10), Verdict::Secret(_)),
            "{secret} must be refused"
        );
    }

    for ordinary in ["id_rsa.pub", "environment.md", "keyboard.txt", "src/env.rs"] {
        assert_eq!(
            policy.file(&path(ordinary), 10),
            Verdict::Captured,
            "{ordinary} is not a secret"
        );
    }
}

#[test]
fn an_explicit_opt_in_re_admits_one_secret_path_and_no_other() {
    let opt_in = RuleSet::new(vec![rule("/app/.env.example", Level::Global)]);
    let policy = CapturePolicy::new(RuleSet::default(), opt_in, DEFAULT_MAX_SIZE, Vec::new());

    assert_eq!(
        policy.file(&path("app/.env.example"), 10),
        Verdict::Captured
    );
    assert!(matches!(
        policy.file(&path("app/.env"), 10),
        Verdict::Secret(_)
    ));
}

#[test]
fn the_users_exclusion_is_reported_before_the_denylist() {
    let rules = RuleSet::new(vec![rule("*.pem", Level::Global)]);
    let policy = CapturePolicy::new(rules, RuleSet::default(), DEFAULT_MAX_SIZE, Vec::new());

    assert!(matches!(
        policy.file(&path("server.pem"), 10),
        Verdict::Excluded(rule) if rule.text == "*.pem"
    ));
}

#[test]
fn a_file_over_the_cap_is_too_large_and_says_by_how_much() {
    let policy = default_policy();

    assert_eq!(
        policy.file(&path("disk.img"), 100 * MIB + 1),
        Verdict::TooLarge {
            size: 100 * MIB + 1,
            max: 100 * MIB,
            rule: None,
        }
    );
    assert_eq!(
        policy.file(&path("exact.img"), 100 * MIB),
        Verdict::Captured
    );
}

#[test]
fn a_per_pattern_cap_overrides_the_scalar_and_the_last_match_wins() {
    let sizes = vec![
        SizeRule {
            rule: rule("*.iso", Level::Global),
            max: 4096 * MIB,
        },
        SizeRule {
            rule: rule("*.log", Level::Global),
            max: MIB,
        },
        SizeRule {
            rule: rule("!keep/*.log", Level::Root),
            max: 0,
        },
    ];
    let policy = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 100 * MIB, sizes);

    assert_eq!(
        policy.file(&path("linux.iso"), 1024 * MIB),
        Verdict::Captured
    );
    assert!(matches!(
        policy.file(&path("debug.log"), 2 * MIB),
        Verdict::TooLarge { max, rule: Some(rule), .. } if max == MIB && rule.text == "*.log"
    ));
    assert_eq!(
        policy.file(&path("keep/debug.log"), 2 * MIB),
        Verdict::Captured,
        "a negated override returns the path to the scalar cap"
    );
}

#[test]
fn reserved_names_are_refused_whatever_the_rules_say() {
    let rules = RuleSet::new(vec![rule("!.git/", Level::Root)]);
    let policy = CapturePolicy::new(rules, RuleSet::default(), DEFAULT_MAX_SIZE, Vec::new());

    for reserved in [".git", "sub/.GIT", "GIT~1", ".git.", ".git "] {
        assert_eq!(
            policy.directory(&path(reserved)),
            Verdict::Reserved,
            "{reserved:?}"
        );
    }
    assert_eq!(policy.file(&path("sub/.git"), 10), Verdict::Reserved);
    assert_eq!(policy.directory(&path(".github")), Verdict::Captured);
}

#[test]
fn explaining_a_path_under_a_pruned_directory_names_the_ancestor_and_the_rule_it_overrode() {
    let rules = RuleSet::new(vec![
        rule("node_modules/", Level::Global),
        rule("!node_modules/mylib/**", Level::Root),
    ]);
    let policy = CapturePolicy::new(rules, RuleSet::default(), DEFAULT_MAX_SIZE, Vec::new());

    let explained = policy.explain(&path("node_modules/mylib/index.js"), Kind::File, Some(10));

    match explained.verdict {
        Verdict::Pruned { ancestor, rule } => {
            assert_eq!(ancestor.as_str(), "node_modules");
            assert_eq!(rule.origin.level, Level::Global);
        }
        other => panic!("expected pruning, got {other:?}"),
    }
    assert_eq!(
        explained.own_rule.expect("its own rule").text,
        "!node_modules/mylib/**"
    );
}

#[test]
fn explaining_a_path_of_unknown_size_never_calls_it_too_large() {
    let policy = default_policy();

    assert_eq!(
        policy
            .explain(&path("missing.bin"), Kind::File, None)
            .verdict,
        Verdict::Captured
    );
}

#[test]
fn a_per_pattern_cap_written_for_a_directory_covers_the_files_beneath_it() {
    let sizes = vec![SizeRule {
        rule: rule("videos/", Level::Global),
        max: 4096 * MIB,
    }];
    let policy = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 100 * MIB, sizes);

    assert_eq!(
        policy.file(&path("videos/2026/holiday.mp4"), 1024 * MIB),
        Verdict::Captured
    );
    assert!(matches!(
        policy.file(&path("other/holiday.mp4"), 1024 * MIB),
        Verdict::TooLarge { .. }
    ));
}

#[test]
fn explaining_an_opted_in_secret_names_both_the_denylist_entry_and_the_opt_in() {
    let opt_in = RuleSet::new(vec![rule("/app/.env.example", Level::Global)]);
    let policy = CapturePolicy::new(RuleSet::default(), opt_in, DEFAULT_MAX_SIZE, Vec::new());

    let explained = policy.explain(&path("app/.env.example"), Kind::File, Some(10));

    assert_eq!(explained.verdict, Verdict::Captured);
    assert_eq!(explained.secret.expect("the denylist entry").text, ".env*");
    assert_eq!(
        explained.opted_in.expect("the opt-in").text,
        "/app/.env.example"
    );
}

#[test]
fn explaining_says_where_the_scalar_cap_was_set() {
    let origin = RuleOrigin {
        level: Level::Root,
        source: ".keepster.toml".to_owned(),
        line: Some(3),
    };
    let policy = CapturePolicy::new(RuleSet::default(), RuleSet::default(), 10, Vec::new())
        .with_max_size_origin(origin.clone());

    let explained = policy.explain(&path("big.bin"), Kind::File, Some(11));

    assert!(matches!(
        explained.verdict,
        Verdict::TooLarge {
            max: 10,
            rule: None,
            ..
        }
    ));
    assert_eq!(explained.max_size, 10);
    assert_eq!(explained.max_size_origin, &origin);
    assert_eq!(
        default_policy()
            .explain(&path("x"), Kind::File, None)
            .max_size_origin
            .level,
        Level::Default,
        "a user who configures nothing gets the built-in cap, and is told so"
    );
}
