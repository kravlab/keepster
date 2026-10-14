//! The platform table's columns (ticket 70): what each degenerate case came to on one machine, written
//! by the catalogue's own test and read back by whatever assembles the table.

use keepster_testkit::degenerate::{CATALOGUE, Outcome};
use keepster_testkit::host::Host;
use keepster_testkit::platform::{
    Column, Gap, Row, gaps, heading, read_columns, report_dir, table,
};
use serde_json::json;

mod common;

use common::scratch;

/// A column made on a runner, by hand, so that nothing in it depends on the machine running this.
fn column() -> Column {
    Column {
        host: Host {
            os: "macos".to_owned(),
            arch: "aarch64".to_owned(),
            version: Some("14.7.1".to_owned()),
            filesystem: Some("apfs".to_owned()),
            runner: Some("macos14 20251006.1".to_owned()),
        },
        rows: vec![
            Row {
                case: "long_name".to_owned(),
                outcome: Outcome::Ran,
            },
            Row {
                case: "non_utf8_name".to_owned(),
                outcome: Outcome::Unavailable("the filesystem refuses it".to_owned()),
            },
            Row {
                case: "past_the_cap".to_owned(),
                outcome: Outcome::Impossible("no such file here".to_owned()),
            },
        ],
    }
}

#[test]
fn a_column_reads_back_as_it_was_written() {
    let directory = scratch();

    let written = column().write(directory.path()).expect("written");

    assert_eq!(Column::read(&written).expect("read"), column());
}

#[test]
fn a_column_is_written_under_the_name_of_its_platform() {
    let directory = scratch();

    let written = column().write(directory.path()).expect("written");

    assert_eq!(written, directory.path().join("platform-macos.json"));
}

#[test]
fn a_column_is_written_into_a_directory_that_is_not_there_yet() {
    let directory = scratch();
    let reports = directory.path().join("target").join("reports");

    let written = column().write(&reports).expect("written");

    assert!(written.starts_with(&reports));
    assert!(written.is_file());
}

#[test]
fn a_row_names_its_case_its_outcome_and_any_reason() {
    let rows = serde_json::to_value(column().rows).expect("serialised");

    assert_eq!(
        rows,
        json!([
            { "case": "long_name", "outcome": "ran" },
            { "case": "non_utf8_name", "outcome": "unavailable", "reason": "the filesystem refuses it" },
            { "case": "past_the_cap", "outcome": "impossible", "reason": "no such file here" }
        ])
    );
}

#[test]
fn what_is_not_a_column_is_refused_rather_than_read_as_an_empty_one() {
    let directory = scratch();
    let path = directory.path().join("platform-linux.json");
    std::fs::write(&path, b"{\"rows\": []}").expect("written");

    let refused = Column::read(&path).expect_err("no host");

    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn without_a_report_directory_nothing_is_written() {
    assert_eq!(report_dir(None).expect("no directory"), None);
}

#[test]
fn an_absolute_report_directory_is_where_the_column_goes() {
    let directory = scratch();

    let named = report_dir(Some(directory.path().as_os_str().to_owned())).expect("a directory");

    assert_eq!(named.as_deref(), Some(directory.path()));
}

#[test]
fn a_relative_report_directory_is_refused() {
    // `cargo test` runs each test in its own crate's directory, so a relative path would name a
    // directory nobody looks in.
    let refused = report_dir(Some("target/reports".into())).expect_err("relative");

    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
    assert!(
        refused.to_string().contains("KEEPSTER_REPORT_DIR"),
        "{refused}"
    );
}

#[test]
fn an_empty_report_directory_is_refused_and_shown_as_empty() {
    // An unset CI input arrives as an empty value; refused, not read as unset, so it is noticed.
    let refused = report_dir(Some("".into())).expect_err("empty");

    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
    assert!(refused.to_string().contains("not \"\""), "{refused}");
}

#[test]
fn an_outcome_that_is_not_one_of_the_three_is_refused() {
    let directory = scratch();
    let path = directory.path().join("platform-linux.json");
    let mut text = serde_json::to_value(column()).expect("serialised");
    text["rows"][0]["outcome"] = json!("skipped");
    std::fs::write(&path, text.to_string()).expect("written");

    let refused = Column::read(&path).expect_err("not an outcome");

    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidData);
}

/// A column for `os`, every catalogued case with `outcome`.
fn everywhere(os: &str, outcome: Outcome) -> Column {
    Column {
        host: Host {
            os: os.to_owned(),
            arch: "x86_64".to_owned(),
            version: None,
            filesystem: None,
            runner: None,
        },
        rows: CATALOGUE
            .iter()
            .map(|case| Row {
                case: case.name.to_owned(),
                outcome: outcome.clone(),
            })
            .collect(),
    }
}

/// `column` with `case`'s outcome replaced.
fn with(mut column: Column, case: &str, outcome: Outcome) -> Column {
    let row = column
        .rows
        .iter_mut()
        .find(|row| row.case == case)
        .expect("catalogued");
    row.outcome = outcome;
    column
}

/// The table's row for `case`, from its first cell to its last.
fn row_of<'a>(table: &'a str, case: &str) -> &'a str {
    table
        .lines()
        .find(|line| line.starts_with(&format!("| `{case}` |")))
        .unwrap_or_else(|| panic!("no row for {case} in:\n{table}"))
}

#[test]
fn a_platform_with_no_column_is_untested_in_every_row() {
    let rendered = table(&[everywhere("linux", Outcome::Ran)]);

    for case in CATALOGUE {
        assert!(
            row_of(&rendered, case.name).ends_with("| ran | untested | untested |"),
            "{}",
            row_of(&rendered, case.name)
        );
    }
}

#[test]
fn the_columns_are_linux_macos_and_windows_whatever_order_they_come_in() {
    let rendered = table(&[
        everywhere("windows", Outcome::Impossible("w".to_owned())),
        everywhere("linux", Outcome::Ran),
        everywhere("macos", Outcome::Unavailable("m".to_owned())),
    ]);

    assert!(
        rendered.contains("| Case | What it is | Linux | macOS | Windows |"),
        "{rendered}"
    );
    assert!(
        row_of(&rendered, "symlinks").ends_with("| ran | unavailable: m | impossible: w |"),
        "{rendered}"
    );
}

#[test]
fn a_reason_cannot_break_its_row() {
    let column = with(
        everywhere("linux", Outcome::Ran),
        "symlinks",
        Outcome::Unavailable("refused | denied\r\nby the system".to_owned()),
    );

    let rendered = table(&[column]);

    assert!(
        row_of(&rendered, "symlinks")
            .ends_with("| unavailable: refused \\| denied by the system | untested | untested |"),
        "{rendered}"
    );
}

#[test]
fn a_case_a_column_does_not_name_is_untested_there() {
    // A column written before the case was catalogued says nothing about it.
    let mut column = everywhere("linux", Outcome::Ran);
    column.rows.retain(|row| row.case != "past_the_cap");

    let rendered = table(&[column]);

    assert!(row_of(&rendered, "past_the_cap").ends_with("| untested | untested | untested |"));
    assert!(row_of(&rendered, "long_name").ends_with("| ran | untested | untested |"));
}

#[test]
fn every_column_says_where_it_ran_and_a_missing_one_says_it_did_not() {
    let rendered = table(&[column()]);

    assert!(
        rendered.contains(
            "- macOS: macos aarch64, version 14.7.1, filesystem apfs, runner macos14 20251006.1"
        ),
        "{rendered}"
    );
    assert!(rendered.contains("- Linux: untested"), "{rendered}");
    assert!(rendered.contains("- Windows: untested"), "{rendered}");
}

#[test]
fn what_a_machine_would_not_say_is_printed_as_unknown() {
    let rendered = table(&[everywhere("linux", Outcome::Ran)]);

    assert!(
        rendered.contains(
            "- Linux: linux x86_64, version unknown, filesystem unknown, not a hosted runner"
        ),
        "{rendered}"
    );
}

#[test]
fn every_case_ran_or_impossible_everywhere_leaves_no_gap() {
    let windows = with(
        everywhere("windows", Outcome::Ran),
        "non_utf8_name",
        Outcome::Impossible("UTF-16 names".to_owned()),
    );

    let found = gaps(&[
        everywhere("linux", Outcome::Ran),
        everywhere("macos", Outcome::Ran),
        windows,
    ]);

    assert_eq!(found, []);
}

#[test]
fn an_unavailable_case_possible_on_linux_is_a_gap() {
    let windows = with(
        everywhere("windows", Outcome::Ran),
        "symlinks",
        Outcome::Unavailable("no Developer Mode".to_owned()),
    );

    let found = gaps(&[
        everywhere("linux", Outcome::Ran),
        everywhere("macos", Outcome::Ran),
        windows,
    ]);

    assert_eq!(
        found,
        [Gap {
            case: "symlinks".to_owned(),
            platform: "windows".to_owned(),
            outcome: Some(Outcome::Unavailable("no Developer Mode".to_owned())),
        }]
    );
}

#[test]
fn an_untested_platform_is_a_gap_for_every_case_possible_on_linux() {
    let linux = with(
        everywhere("linux", Outcome::Ran),
        "long_name",
        Outcome::Impossible("hypothetically".to_owned()),
    );

    let found = gaps(&[linux, everywhere("windows", Outcome::Ran)]);

    let expected: Vec<Gap> = CATALOGUE
        .iter()
        .filter(|case| case.name != "long_name")
        .map(|case| Gap {
            case: case.name.to_owned(),
            platform: "macos".to_owned(),
            outcome: None,
        })
        .collect();
    assert_eq!(found, expected);
}

#[test]
fn without_a_linux_column_nothing_is_known_possible_so_every_case_is_a_gap() {
    let found = gaps(&[
        everywhere("macos", Outcome::Ran),
        everywhere("windows", Outcome::Ran),
    ]);

    assert_eq!(found.len(), CATALOGUE.len());
    assert!(
        found
            .iter()
            .all(|gap| gap.platform == "linux" && gap.outcome.is_none())
    );
}

#[test]
fn the_columns_of_a_directory_are_its_platform_files_and_nothing_else() {
    let directory = scratch();
    column().write(directory.path()).expect("written");
    everywhere("linux", Outcome::Ran)
        .write(directory.path())
        .expect("written");
    std::fs::write(directory.path().join("conformance-linux.md"), b"# other").expect("written");

    let mut read = read_columns(directory.path()).expect("read");
    read.sort_by(|a, b| a.host.os.cmp(&b.host.os));

    assert_eq!(read, [everywhere("linux", Outcome::Ran), column()]);
}

#[test]
fn a_directory_without_columns_has_none() {
    let directory = scratch();

    assert_eq!(read_columns(directory.path()).expect("read"), []);
}

#[test]
fn a_platform_is_headed_as_the_table_heads_it_and_an_unknown_one_by_its_own_name() {
    assert_eq!(heading("macos"), "macOS");
    assert_eq!(heading("freebsd"), "freebsd");
}
