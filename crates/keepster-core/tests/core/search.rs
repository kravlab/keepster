//! What a search matches (ticket 59): a path by a fragment or an Include Rules pattern, content by a
//! literal or a regular expression — case-insensitively unless the query has a capital — and nothing a
//! query could make take forever.

use keepster_core::{ContentQuery, PathQuery, RootPath, is_binary};

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a Root-relative path")
}

#[test]
fn a_path_is_found_by_a_fragment_of_it_case_ignored_unless_the_query_has_capitals() {
    let lower = PathQuery::new("config", false).expect("a query");
    assert!(lower.matches(&rp("app/Config.toml")));
    assert!(lower.matches(&rp("config.toml")));
    assert!(!lower.matches(&rp("app/main.rs")));

    let capital = PathQuery::new("Config", false).expect("a query");
    assert!(capital.matches(&rp("app/Config.toml")));
    assert!(
        !capital.matches(&rp("config.toml")),
        "a capital asks for case"
    );
}

#[test]
fn a_pattern_is_read_as_an_include_rule_is() {
    let pattern = PathQuery::new("**/*.env", true).expect("a pattern");
    assert!(pattern.matches(&rp("deploy/prod.env")));
    assert!(!pattern.matches(&rp("deploy/prod.envy")));

    let anywhere = PathQuery::new("*.toml", true).expect("a pattern");
    assert!(
        anywhere.matches(&rp("a/b/c.toml")),
        "a pattern without a slash matches at any depth"
    );

    let folded = PathQuery::new("*.env", true).expect("a pattern");
    assert!(
        folded.matches(&rp("deploy/PROD.ENV")),
        "a pattern with no capital ignores case"
    );
    let cased = PathQuery::new("*.ENV", true).expect("a pattern");
    assert!(cased.matches(&rp("PROD.ENV")));
    assert!(!cased.matches(&rp("prod.env")), "a capital asks for case");

    let directory = PathQuery::new("build/", true).expect("a pattern");
    assert!(
        directory.matches(&rp("build/out/app.o")),
        "what a directory pattern takes in is found, as an Include Rule takes it in"
    );
    assert!(
        !directory.matches(&rp("build")),
        "a file is not a directory"
    );
    assert!(!directory.matches(&rp("src/main.rs")));

    assert!(
        PathQuery::new("!*.toml", true).is_err(),
        "a negation names nothing to find"
    );
    assert!(PathQuery::new("", false).is_err());
}

#[test]
fn content_is_matched_line_by_line_literally_by_default() {
    let query = ContentQuery::new("a.b", false).expect("a query");
    let found = query.lines(b"first\nfind a.b here\naxb is no match\r\nA.B too\n");

    let lines: Vec<(u64, &str)> = found
        .iter()
        .map(|found| (found.line, found.snippet.as_str()))
        .collect();
    assert_eq!(lines, [(2, "find a.b here"), (4, "A.B too")]);
}

#[test]
fn a_regular_expression_is_used_as_one_and_smart_case_holds_for_it_too() {
    let query = ContentQuery::new(r"fn \w+_test", true).expect("an expression");
    let found = query.lines(b"fn parse_test() {}\nFN LOUD_TEST\nlet x = 1;\n");
    assert_eq!(found.len(), 2, "lower case ignores case: {found:?}");

    let capital = ContentQuery::new(r"FN \w+", true).expect("an expression");
    assert_eq!(capital.lines(b"fn quiet\nFN LOUD\n").len(), 1);

    let ending = ContentQuery::new("done$", true).expect("an expression");
    assert_eq!(
        ending.lines(b"not done\r\ndone yet\r\nall done\r\n").len(),
        2,
        "`$` is the end of the line, whichever ending it has"
    );
    let empty = ContentQuery::new("^$", true).expect("an expression");
    let found: Vec<u64> = empty
        .lines(b"a\n\nb\n")
        .iter()
        .map(|found| found.line)
        .collect();
    assert_eq!(found, [2], "no line after the last line ending");

    let escaped = ContentQuery::new(r"let\s+\S+ = \pL\p{Greek}", true).expect("an expression");
    assert_eq!(
        escaped.lines("LET x = aβ\n".as_bytes()).len(),
        1,
        "an escape is not a capital"
    );

    assert!(ContentQuery::new("(unclosed", true).is_err());
}

#[test]
fn an_expression_meant_to_backtrack_forever_is_answered_at_once() {
    // Catastrophic for a backtracking engine; linear for this one.
    let query = ContentQuery::new("(a+)+$", true).expect("an expression");
    let content = format!("{}!", "a".repeat(100_000));

    let started = std::time::Instant::now();
    assert!(query.lines(content.as_bytes()).is_empty());
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn a_long_line_is_shown_around_what_matched() {
    let line = format!("{}needle{}", "x".repeat(1000), "y".repeat(1000));
    let query = ContentQuery::new("needle", false).expect("a query");

    let found = query.lines(line.as_bytes());

    assert_eq!(found.len(), 1);
    assert!(found[0].snippet.contains("needle"), "{}", found[0].snippet);
    assert!(
        found[0].snippet.chars().count() <= 162,
        "{}",
        found[0].snippet
    );
    assert!(found[0].snippet.starts_with('…') && found[0].snippet.ends_with('…'));
}

#[test]
fn a_nul_among_the_first_bytes_makes_a_version_binary() {
    assert!(is_binary(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"));
    assert!(!is_binary(b"plain text\n"));
    assert!(!is_binary(b""));
}

#[test]
fn a_path_that_is_not_utf8_is_found_by_the_text_around_its_stray_bytes() {
    let odd = RootPath::parse_bytes(b"Docs/\xff report.txt").expect("valid");

    assert!(
        PathQuery::new("report", false)
            .expect("a query")
            .matches(&odd)
    );
    assert!(
        PathQuery::new("docs/*.txt", true)
            .expect("a pattern")
            .matches(&odd)
    );
    assert!(
        !PathQuery::new("*.md", true)
            .expect("a pattern")
            .matches(&odd)
    );
}
