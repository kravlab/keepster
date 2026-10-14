//! The report layer's one command (ticket 70). Every report gates nothing; each is written into the
//! directory named on the command line and printed.
//!
//! ```text
//! keepster-report table <dir> [--release]
//! keepster-report conformance <dir>
//! keepster-report coverage <export.json> <dir>
//! ```
//!
//! `table` assembles the platform table from the columns the catalogue's test wrote into `<dir>`, and
//! writes it there as `platform-table.md`. With `--release` it also lists the gaps the release checklist
//! forbids and exits 1 when there is one. `conformance` watches each documented class of change being made
//! in a temporary directory inside `<dir>` and writes what this platform's watcher delivered as
//! `conformance-<os>.md`; a class the machine could not act out is a line in it, not a failure.
//! `coverage` reads what `cargo llvm-cov --json --summary-only` exported and writes which crate of this
//! workspace no test reaches as `coverage-<os>.md`. Exit 2 is a mistake in the command or a file that
//! cannot be read or written.
//!
//! All of the work is in [`keepster_testkit::platform`], [`keepster_testkit::conformance`] and
//! [`keepster_testkit::coverage`]; this only reads the arguments and the files.

use std::path::Path;
use std::process::ExitCode;

use keepster_testkit::conformance::{CLASSES, observe};
use keepster_testkit::coverage::{crates_in, summarize};
use keepster_testkit::platform::{gaps, heading, read_columns, table};
use keepster_testkit::{conformance, coverage, host};

/// How to call this, printed with a mistake in the command.
const USAGE: &str = "usage: keepster-report table <dir> [--release] | conformance <dir> | coverage <export.json> <dir>";

/// Run the report the arguments name: exit 0 when it was made, 1 when `table --release` found a gap,
/// 2 when the command is wrong or a file cannot be read or written.
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let outcome = match args.as_slice() {
        ["table", directory] => platform_table(Path::new(directory), false),
        ["table", directory, "--release"] => platform_table(Path::new(directory), true),
        ["conformance", directory] => watcher_conformance(Path::new(directory)),
        ["coverage", export, directory] => crate_coverage(Path::new(export), Path::new(directory)),
        _ => Err(USAGE.to_owned()),
    };
    match outcome {
        Ok(code) => code,
        Err(why) => {
            eprintln!("keepster-report: {why}");
            ExitCode::from(2)
        }
    }
}

/// Assemble, write and print the platform table of `directory`; under `release`, list its gaps and fail
/// when there is one.
fn platform_table(directory: &Path, release: bool) -> Result<ExitCode, String> {
    let columns = read_columns(directory).map_err(|error| {
        format!(
            "the columns in {} cannot be read: {error}",
            directory.display()
        )
    })?;
    publish(
        directory,
        "platform-table.md",
        &table(&columns),
        "platform table",
    )?;
    if !release {
        return Ok(ExitCode::SUCCESS);
    }

    let found = gaps(&columns);
    for gap in &found {
        let heading = heading(&gap.platform);
        match &gap.outcome {
            Some(outcome) => println!("gap: `{}` on {heading}: {outcome}", gap.case),
            None => println!("gap: `{}` on {heading}: untested", gap.case),
        }
    }
    if found.is_empty() {
        println!("keepster-report: no gaps among the cases possible on the reference platform");
        Ok(ExitCode::SUCCESS)
    } else {
        println!(
            "keepster-report: {} gaps; the release checklist allows none",
            found.len()
        );
        Ok(ExitCode::from(1))
    }
}

/// Observe every class of change on this machine, in `directory`, and write and print the report. A
/// class that could not be observed is recorded as such, and the rest are still observed.
fn watcher_conformance(directory: &Path) -> Result<ExitCode, String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{} cannot be made: {error}", directory.display()))?;
    let observations: Vec<_> = CLASSES
        .iter()
        .map(|class| {
            let observation = observe(class, directory).map_err(|error| error.to_string());
            (class, observation)
        })
        .collect();
    let text = conformance::render(&host::detect(directory), &observations);
    let name = format!("conformance-{}.md", std::env::consts::OS);
    publish(directory, &name, &text, "watcher conformance")?;
    Ok(ExitCode::SUCCESS)
}

/// Summarize the coverage `export` per crate of this workspace, and write and print the report into
/// `directory`.
fn crate_coverage(export: &Path, directory: &Path) -> Result<ExitCode, String> {
    // This tool is built and run inside the workspace, so its own crate's parent holds every crate.
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("this crate is in no directory")?;
    let names = crates_in(crates)
        .map_err(|error| format!("{} cannot be listed: {error}", crates.display()))?;
    let text = std::fs::read_to_string(export)
        .map_err(|error| format!("{} cannot be read: {error}", export.display()))?;
    let summary = summarize(&names, &text)
        .map_err(|error| format!("{} is not a coverage export: {error}", export.display()))?;
    let name = format!("coverage-{}.md", std::env::consts::OS);
    publish(directory, &name, &coverage::render(&summary), "coverage")?;
    Ok(ExitCode::SUCCESS)
}

/// Write the report `text` into `directory`, made if it is not there, as `name`; then print it and where
/// the `what` report was written.
fn publish(directory: &Path, name: &str, text: &str, what: &str) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("{} cannot be made: {error}", directory.display()))?;
    let path = directory.join(name);
    std::fs::write(&path, text)
        .map_err(|error| format!("{} cannot be written: {error}", path.display()))?;
    print!("{text}");
    println!("\nkeepster-report: {what} written to {}", path.display());
    Ok(())
}
