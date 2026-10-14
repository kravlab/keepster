//! The platform table of the degenerate catalogue (ticket 70): per platform, what each case came to —
//! ran, impossible here, or unavailable on this machine — so that a skip is never mistaken for coverage.
//!
//! The catalogue's own test writes one [`Column`] per run, for the machine it ran on, when
//! `KEEPSTER_REPORT_DIR` names where; whatever assembles the table reads the columns of every platform
//! that ran. A column is JSON, written and read by this module alone, and never committed: it carries
//! the machine it was made on.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::degenerate::{CATALOGUE, Outcome};
use crate::host::Host;

/// The platforms a column can be for, as `std::env::consts::OS` names them, with their headings, in
/// the table's order. Linux comes first: it is the reference platform (ticket 30).
pub const PLATFORMS: [(&str, &str); 3] = [
    ("linux", "Linux"),
    ("macos", "macOS"),
    ("windows", "Windows"),
];

/// The heading the table gives the platform `os`, as `std::env::consts::OS` names it; a platform the
/// table has no column for is headed by that name.
pub fn heading(os: &str) -> &str {
    PLATFORMS
        .iter()
        .find(|(name, _)| *name == os)
        .map_or(os, |(_, heading)| heading)
}

/// The reference platform, whose possible cases the release checklist requires everywhere (ticket 31).
const REFERENCE: &str = "linux";

/// The variable naming the directory a run writes its reports into; unset, nothing is written.
pub const REPORT_DIR: &str = "KEEPSTER_REPORT_DIR";

/// The directory [`REPORT_DIR`]'s `value` names, `None` when it is unset.
///
/// # Errors
///
/// [`io::ErrorKind::InvalidInput`] for a relative path: `cargo test` runs each test binary in its own
/// crate's directory, so a relative one would name a directory beneath that crate nobody looks in.
pub fn report_dir(value: Option<OsString>) -> io::Result<Option<PathBuf>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let directory = PathBuf::from(value);
    if directory.is_absolute() {
        Ok(Some(directory))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{REPORT_DIR} must be an absolute path, not {:?}: tests run in their own crate's directory",
                directory.display().to_string()
            ),
        ))
    }
}

/// One platform's column: the machine one run of the catalogue's test was made on, and what each case
/// came to there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    /// The machine.
    pub host: Host,
    /// One row per case, in the catalogue's order.
    pub rows: Vec<Row>,
}

/// What one case came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Row {
    /// The case's name, as the catalogue keys it.
    pub case: String,
    /// Its outcome, with the reason where it has one.
    #[serde(flatten)]
    pub outcome: Outcome,
}

impl Column {
    /// Write the column into `directory`, made if it is not there, as `platform-<os>.json`: one file per
    /// platform, so a later run on the same one — another architecture or filesystem included — replaces
    /// it. The table has one column per platform, not per machine.
    ///
    /// # Errors
    ///
    /// When the directory cannot be made or the file cannot be written.
    pub fn write(&self, directory: &Path) -> io::Result<PathBuf> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("platform-{}.json", self.host.os));
        let mut text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        text.push('\n');
        std::fs::write(&path, text)?;

        Ok(path)
    }

    /// The column written at `path`.
    ///
    /// # Errors
    ///
    /// When the file cannot be read, or [`io::ErrorKind::InvalidData`] when it is not a column.
    pub fn read(path: &Path) -> io::Result<Self> {
        let text = std::fs::read(path)?;

        serde_json::from_slice(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

/// Every column written into `directory`: its `platform-*.json` files, whatever else it holds.
///
/// # Errors
///
/// When the directory cannot be listed, or a column in it cannot be read (see [`Column::read`]).
pub fn read_columns(directory: &Path) -> io::Result<Vec<Column>> {
    let mut columns = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        let named = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("platform-") && name.ends_with(".json"));
        if named {
            columns.push(Column::read(&path)?);
        }
    }
    Ok(columns)
}

/// A hole in the table the release checklist does not allow: a case possible on the reference platform
/// that some platform did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    /// The case.
    pub case: String,
    /// The platform, as `std::env::consts::OS` names it.
    pub platform: String,
    /// What it came to there, `None` when that platform has no outcome for it: untested.
    pub outcome: Option<Outcome>,
}

/// The platform table of `columns`, in Markdown: a row per catalogued case, a column per platform, each
/// cell the case's outcome there or `untested` where no column says, and beneath it the machine each
/// column ran on. It gates nothing: an `unavailable` or `untested` cell is printed, never refused.
pub fn table(columns: &[Column]) -> String {
    let mut text = String::from(
        "# Platform table\n\n\
         What each degenerate case came to on each platform: ran, impossible there (decided when the code \
         was built), unavailable on the machine that ran it, or untested where no run said.\n\n\
         | Case | What it is |",
    );
    for (_, heading) in PLATFORMS {
        text.push_str(&format!(" {heading} |"));
    }
    text.push_str("\n| --- | --- |");
    for _ in PLATFORMS {
        text.push_str(" --- |");
    }
    text.push('\n');
    for case in CATALOGUE {
        text.push_str(&format!("| `{}` | {} |", case.name, cell(case.summary)));
        for (os, _) in PLATFORMS {
            let shown = match outcome(columns, os, case.name) {
                Some(outcome) => outcome.to_string(),
                None => "untested".to_owned(),
            };
            text.push_str(&format!(" {} |", cell(&shown)));
        }
        text.push('\n');
    }
    text.push_str("\n## Where each column ran\n\n");
    for (os, heading) in PLATFORMS {
        match column(columns, os) {
            Some(column) => text.push_str(&format!(
                "- {heading}: {}\n",
                cell(&column.host.to_string())
            )),
            None => text.push_str(&format!("- {heading}: untested\n")),
        }
    }
    text
}

/// The gaps the release checklist forbids in `columns`: for every case the reference platform does not
/// record as impossible, each platform where it is unavailable or untested. Without a reference column
/// nothing is known to be possible there, so every case is a gap on it.
pub fn gaps(columns: &[Column]) -> Vec<Gap> {
    let mut found = Vec::new();
    for case in CATALOGUE {
        if matches!(
            outcome(columns, REFERENCE, case.name),
            Some(Outcome::Impossible(_))
        ) {
            continue;
        }
        for (os, _) in PLATFORMS {
            let outcome = outcome(columns, os, case.name);
            if !matches!(outcome, Some(Outcome::Ran | Outcome::Impossible(_))) {
                found.push(Gap {
                    case: case.name.to_owned(),
                    platform: os.to_owned(),
                    outcome: outcome.cloned(),
                });
            }
        }
    }
    found
}

/// The column for the platform `os`: the last of them, should several be given.
fn column<'a>(columns: &'a [Column], os: &str) -> Option<&'a Column> {
    columns.iter().rev().find(|column| column.host.os == os)
}

/// What `case` came to on the platform `os`, `None` when no column for it names the case.
fn outcome<'a>(columns: &'a [Column], os: &str, case: &str) -> Option<&'a Outcome> {
    column(columns, os)?
        .rows
        .iter()
        .find(|row| row.case == case)
        .map(|row| &row.outcome)
}

/// `text` as one Markdown table cell: a `|` would end the cell and a line break the row, and both come
/// from operating systems' messages, which nobody here writes.
fn cell(text: &str) -> String {
    text.split(['\r', '\n'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}
