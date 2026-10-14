//! No test prints a skip (ticket 70): nobody reads `cargo test`'s output, so a test that prints "skipped"
//! and passes reads as green on a platform where it never ran (ticket 31). A case a platform cannot hold
//! is compiled out with its reason, and one a machine cannot make is reported in the platform table.
//!
//! The forbidden spellings below are built with `concat!`, so this file holds none of them itself.

use std::path::{Path, PathBuf};

use keepster_testkit::skips::{Finding, printed_skips};

#[test]
fn a_test_file_that_prints_is_found_at_the_line_it_prints() {
    let source = concat!(
        "#[test]\n",
        "fn a() {\n",
        "    if missing() {\n",
        "        eprin",
        "tln!(\"skipped: no git\");\n",
        "        return;\n",
        "    }\n",
        "}\n",
    );

    let found = printed_skips(source, true);

    assert_eq!(
        found,
        [Finding {
            line: 4,
            text: concat!("eprin", "tln!(\"skipped: no git\");").to_owned()
        }]
    );
}

#[test]
fn a_test_only_item_without_a_body_does_not_make_the_next_function_a_test_module() {
    let source = concat!(
        "#[cfg(test)]\n",
        "use std::fmt;\n",
        "\n",
        "fn main() {\n",
        "    prin",
        "tln!(\"the product prints what it is for\");\n",
        "}\n",
    );

    assert_eq!(printed_skips(source, false), []);
}

#[test]
fn every_printing_macro_and_an_ignored_test_are_found() {
    let source = concat!(
        "pri",
        "nt!(\"a\");\n",
        "prin",
        "tln!(\"b\");\n",
        "epri",
        "nt!(\"c\");\n",
        "db",
        "g!(value);\n",
        "#[",
        "ignore]\n",
        "#[",
        "ignore = \"slow\"]\n",
    );

    let lines: Vec<usize> = printed_skips(source, true)
        .iter()
        .map(|finding| finding.line)
        .collect();

    assert_eq!(lines, [1, 2, 3, 4, 5, 6]);
}

#[test]
fn a_comment_that_names_a_macro_is_not_a_print() {
    let source = concat!(
        "// a test never calls prin",
        "tln!\n",
        "let x = 1; // nor eprin",
        "tln!\n",
    );

    assert_eq!(printed_skips(source, true), []);
}

#[test]
fn a_name_that_merely_ends_like_a_macro_is_not_one() {
    let source = concat!("my_prin", "tln!(\"a\");\n", "repri", "nt!(\"b\");\n");

    assert_eq!(printed_skips(source, true), []);
}

#[test]
fn in_a_source_file_only_its_test_module_is_the_suite() {
    let source = concat!(
        "pub fn report() {\n",
        "    prin",
        "tln!(\"what the command prints\");\n",
        "}\n",
        "\n",
        "#[cfg(test)]\n",
        "mod tests {\n",
        "    #[test]\n",
        "    fn a() {\n",
        "        eprin",
        "tln!(\"skipped\");\n",
        "    }\n",
        "}\n",
        "\n",
        "pub fn after() {\n",
        "    prin",
        "tln!(\"also the command's\");\n",
        "}\n",
    );

    let lines: Vec<usize> = printed_skips(source, false)
        .iter()
        .map(|finding| finding.line)
        .collect();

    assert_eq!(lines, [9]);
}

#[test]
fn no_test_in_the_workspace_prints() {
    let mut found = Vec::new();
    let mut scanned = Vec::new();
    for crate_dir in children(&workspace().join("crates")) {
        for (directory, whole) in [("tests", true), ("src", false)] {
            for file in rust_files(&crate_dir.join(directory)) {
                scanned.push(file.clone());
                let source = std::fs::read_to_string(&file).expect("a source file");
                for finding in printed_skips(&source, whole) {
                    found.push(format!(
                        "{}:{}: {}",
                        file.display(),
                        finding.line,
                        finding.text
                    ));
                }
            }
        }
    }

    // The walk reached the suite: this very file is among what it read.
    assert!(
        scanned.contains(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("printed_skips.rs")
        ),
        "{scanned:?}"
    );
    assert!(
        found.is_empty(),
        "a test may not print a skip — compile it out with its reason, or report it in the platform \
         table (ticket 70):\n{}",
        found.join("\n")
    );
}

/// The workspace root, two directories above this crate.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate lives two directories below the workspace root")
        .to_path_buf()
}

/// The directories directly in `directory`.
fn children(directory: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .expect("a directory")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| path.is_dir())
        .collect()
}

/// Every `.rs` file beneath `directory`, none when it is not there.
fn rust_files(directory: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files
}
