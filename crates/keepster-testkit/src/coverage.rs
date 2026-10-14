//! The coverage report (ticket 70): per crate, how many lines of its own source any test reaches, read
//! from what `cargo llvm-cov --json --summary-only` exports. It answers one question, which crate no
//! test reaches at all, and has no threshold: it gates nothing.
//!
//! A crate's own source is every file under `crates/<name>/src/`; its tests, benches and every
//! dependency's source are not counted.

use std::io;
use std::path::Path;

use serde::Deserialize;

/// One crate's lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crate {
    /// Its directory's name under `crates/`.
    pub name: String,
    /// The lines of its source the export measured; 0 when it measured none.
    pub lines: u64,
    /// How many of them a test reached.
    pub covered: u64,
}

/// What of `llvm-cov export --summary-only` is read: every other field is left alone.
/// The export as a whole.
#[derive(Deserialize)]
struct Export {
    data: Vec<Data>,
}

/// One export's worth of files; `cargo llvm-cov` writes one.
#[derive(Deserialize)]
struct Data {
    files: Vec<File>,
}

/// One source file, named as the platform that ran the export names it.
#[derive(Deserialize)]
struct File {
    filename: String,
    summary: Summary,
}

/// What was measured in one file; only its lines are read.
#[derive(Deserialize)]
struct Summary {
    lines: Lines,
}

/// One file's lines: how many were measured and how many a test reached.
#[derive(Deserialize)]
struct Lines {
    count: u64,
    covered: u64,
}

/// The lines of each crate in `crates`, in that order, from the export `text`. A crate the export does
/// not name has no line measured, which is how a crate no test binary links shows up.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidData`] when `text` is not such an export.
pub fn summarize(crates: &[String], text: &str) -> io::Result<Vec<Crate>> {
    let export: Export = serde_json::from_str(text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut summary: Vec<Crate> = crates
        .iter()
        .map(|name| Crate {
            name: name.clone(),
            lines: 0,
            covered: 0,
        })
        .collect();
    for file in export.data.iter().flat_map(|data| &data.files) {
        let Some(owner) = owner(&file.filename) else {
            continue;
        };
        if let Some(each) = summary.iter_mut().find(|each| each.name == owner) {
            each.lines += file.summary.lines.count;
            each.covered += file.summary.lines.covered;
        }
    }
    Ok(summary)
}

/// The crate whose own source `filename` is: the `<name>` of its last `crates/<name>/src/`, with either
/// separator, since the export names files as the platform that ran it does.
fn owner(filename: &str) -> Option<&str> {
    let parts: Vec<&str> = filename.split(['/', '\\']).collect();
    parts
        .windows(3)
        .rev()
        .find(|window| window[0] == "crates" && window[2] == "src")
        .map(|window| window[1])
}

/// The names of the crates under `directory`: each directory there with a `Cargo.toml`, sorted.
///
/// # Errors
///
/// When `directory` cannot be listed.
pub fn crates_in(directory: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if !path.join("Cargo.toml").is_file() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The coverage report of `summary` in Markdown: a row per crate with its lines, those reached and their
/// share, then the crates no test reaches, those with no line measured among them.
pub fn render(summary: &[Crate]) -> String {
    let mut text = String::from(
        "# Coverage\n\n\
         The lines of each crate's own source that the workspace's tests reach. There is no threshold: \
         the one question is which crate no test reaches at all.\n\n\
         | Crate | Lines | Reached | Share |\n| --- | --- | --- | --- |\n",
    );
    for each in summary {
        let share = if each.lines == 0 {
            "none measured".to_owned()
        } else {
            let share = each.covered as f64 * 100.0 / each.lines as f64;
            format!("{share:.1}%")
        };
        text.push_str(&format!(
            "| `{}` | {} | {} | {share} |\n",
            each.name, each.lines, each.covered
        ));
    }
    text.push_str("\n## Crates no test reaches\n\n");
    let unreached: Vec<&Crate> = summary.iter().filter(|each| each.covered == 0).collect();
    if unreached.is_empty() {
        text.push_str("- none\n");
    }
    for each in unreached {
        text.push_str(&format!("- `{}`\n", each.name));
    }
    text
}
