//! The coverage report (ticket 70): which crate has no line any test reaches, read from what
//! `cargo llvm-cov --json --summary-only` exports. There is no threshold: the report gates nothing.

use keepster_testkit::coverage::{Crate, crates_in, render, summarize};
use serde_json::json;

mod common;

use common::scratch;

/// One file of an export, as `llvm-cov export --summary-only` writes it.
fn file(filename: &str, count: u64, covered: u64) -> serde_json::Value {
    json!({
        "filename": filename,
        "summary": {
            "lines": { "count": count, "covered": covered, "percent": 0.0 },
            "functions": { "count": 1, "covered": 0, "percent": 0.0 }
        }
    })
}

/// An export holding `files`.
fn export(files: Vec<serde_json::Value>) -> String {
    json!({
        "data": [{ "files": files, "totals": {} }],
        "type": "llvm.coverage.json.export",
        "version": "2.0.1"
    })
    .to_string()
}

/// `crates` as the owned names `summarize` takes.
fn names(crates: &[&str]) -> Vec<String> {
    crates.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn the_lines_of_a_crate_are_the_sum_of_its_source_files() {
    let text = export(vec![
        file("/w/crates/keepster-core/src/lib.rs", 10, 4),
        file("/w/crates/keepster-core/src/model/root.rs", 30, 30),
    ]);

    let summary = summarize(&names(&["keepster-core"]), &text).expect("an export");

    assert_eq!(
        summary,
        [Crate {
            name: "keepster-core".to_owned(),
            lines: 40,
            covered: 34,
        }]
    );
}

#[test]
fn a_windows_path_names_its_crate_too() {
    let text = export(vec![file(r"D:\a\w\crates\keepster-cli\src\main.rs", 8, 2)]);

    let summary = summarize(&names(&["keepster-cli"]), &text).expect("an export");

    assert_eq!(summary[0].covered, 2);
    assert_eq!(summary[0].lines, 8);
}

#[test]
fn what_is_not_a_crates_own_source_is_not_counted() {
    let text = export(vec![
        file("/w/crates/keepster-core/tests/model.rs", 50, 50),
        file("/w/crates/keepster-core/benches/walk.rs", 5, 5),
        file(
            "/home/u/.cargo/registry/src/index.crates.io-0/serde-1.0.229/src/lib.rs",
            9,
            9,
        ),
        file("/w/crates/keepster-core/src/lib.rs", 3, 0),
    ]);

    let summary = summarize(&names(&["keepster-core"]), &text).expect("an export");

    assert_eq!(
        summary,
        [Crate {
            name: "keepster-core".to_owned(),
            lines: 3,
            covered: 0,
        }]
    );
}

#[test]
fn a_crate_the_export_does_not_name_has_no_line_measured() {
    // A crate no test binary links is absent from the export altogether, which is the answer, not a
    // reason to leave it out.
    let text = export(vec![file("/w/crates/keepster-core/src/lib.rs", 3, 3)]);

    let summary = summarize(&names(&["keepster-core", "keepster-tray"]), &text).expect("an export");

    assert_eq!(
        summary[1],
        Crate {
            name: "keepster-tray".to_owned(),
            lines: 0,
            covered: 0,
        }
    );
}

#[test]
fn the_crates_come_in_the_order_they_were_named() {
    let text = export(vec![
        file("/w/crates/b/src/lib.rs", 1, 1),
        file("/w/crates/a/src/lib.rs", 1, 1),
    ]);

    let summary = summarize(&names(&["b", "a"]), &text).expect("an export");

    let order: Vec<&str> = summary.iter().map(|each| each.name.as_str()).collect();
    assert_eq!(order, ["b", "a"]);
}

#[test]
fn what_is_not_an_export_is_refused() {
    let refused = summarize(&names(&["a"]), "{\"data\": [{}]}").expect_err("no files");

    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidData);
}

#[test]
fn the_report_lists_every_crate_and_names_those_with_nothing_covered() {
    let report = render(&[
        Crate {
            name: "keepster-core".to_owned(),
            lines: 200,
            covered: 150,
        },
        Crate {
            name: "keepster-tray".to_owned(),
            lines: 40,
            covered: 0,
        },
        Crate {
            name: "keepster-ui".to_owned(),
            lines: 0,
            covered: 0,
        },
    ]);

    assert!(
        report.contains("| `keepster-core` | 200 | 150 | 75.0% |\n"),
        "{report}"
    );
    assert!(
        report.contains("| `keepster-tray` | 40 | 0 | 0.0% |\n"),
        "{report}"
    );
    assert!(
        report.contains("| `keepster-ui` | 0 | 0 | none measured |\n"),
        "{report}"
    );
    assert!(
        report.contains("## Crates no test reaches\n\n- `keepster-tray`\n- `keepster-ui`\n"),
        "{report}"
    );
}

#[test]
fn when_every_crate_is_reached_the_report_says_so() {
    let report = render(&[Crate {
        name: "keepster-core".to_owned(),
        lines: 1,
        covered: 1,
    }]);

    assert!(
        report.contains("## Crates no test reaches\n\n- none\n"),
        "{report}"
    );
}

#[test]
fn the_crates_of_a_directory_are_its_directories_with_a_manifest() {
    let directory = scratch();
    for name in ["keepster-cli", "keepster-core"] {
        std::fs::create_dir(directory.path().join(name)).expect("a crate");
        std::fs::write(directory.path().join(name).join("Cargo.toml"), "").expect("its manifest");
    }
    std::fs::create_dir(directory.path().join("notes")).expect("not a crate");
    std::fs::write(directory.path().join("README.md"), "").expect("not a crate");

    assert_eq!(
        crates_in(directory.path()).expect("listed"),
        ["keepster-cli", "keepster-core"]
    );
}
