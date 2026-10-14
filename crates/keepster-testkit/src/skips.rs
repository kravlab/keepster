//! Finding a printed runtime skip in the suite (ticket 70), which ticket 31 forbids: a test that prints
//! "skipped" and returns reads as green where it never ran, since nobody reads `cargo test`'s output.
//!
//! What is looked for is any print — `print!`, `println!`, `eprint!`, `eprintln!`, `dbg!` — and any
//! `#[ignore]`, because a test has no other reason to print, and an ignored test is a skip decided by
//! hand. The scan reads text, not syntax: a comment is cut at `//`, and a source file's test module is
//! found by counting braces from its `#[cfg(test)]`, which format strings keep balanced. Both can only
//! make it miss something, never report what is not there.

/// The macros a test may not call.
const PRINTS: [&str; 5] = ["print!", "println!", "eprint!", "eprintln!", "dbg!"];

/// The attribute a test may not carry.
const IGNORE: &str = "#[ignore";

/// One forbidden line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Its number, counted from 1.
    pub line: usize,
    /// The line, without its indentation.
    pub text: String,
}

/// The prints and ignored tests in `source`: all of it when `whole` — a file under a crate's `tests` —
/// and otherwise only its `#[cfg(test)]` modules, since the rest of a source file is the product, which
/// may print what it is for.
pub fn printed_skips(source: &str, whole: bool) -> Vec<Finding> {
    let mut found = Vec::new();
    let mut depth: Option<usize> = None;
    let mut pending = false;
    for (index, line) in source.lines().enumerate() {
        let code = line.split("//").next().unwrap_or_default();
        let in_suite = whole || depth.is_some() || pending;
        if in_suite && forbidden(code) {
            found.push(Finding {
                line: index + 1,
                text: line.trim().to_owned(),
            });
        }
        if whole {
            continue;
        }
        if code.trim_start().starts_with("#[cfg(test)]") {
            pending = true;
        }
        for character in code.chars() {
            match (character, depth) {
                ('{', Some(open)) => depth = Some(open + 1),
                ('{', None) if pending => {
                    pending = false;
                    depth = Some(1);
                }
                // A test-only item that ends before any body, such as a `use`, opens no test module.
                (';', None) if pending => pending = false,
                ('}', Some(1)) => depth = None,
                ('}', Some(open)) => depth = Some(open - 1),
                _ => {}
            }
        }
    }
    found
}

/// Whether `code` calls a print or ignores a test.
fn forbidden(code: &str) -> bool {
    code.contains(IGNORE)
        || PRINTS.iter().any(|name| {
            code.match_indices(name).any(|(at, _)| {
                !code[..at]
                    .chars()
                    .next_back()
                    .is_some_and(|before| before.is_alphanumeric() || before == '_')
            })
        })
}
