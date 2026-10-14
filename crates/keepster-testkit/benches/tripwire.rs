//! The operation tripwire (ticket 30): `make bench`, `make bench-calibrate` and `make bench-ceiling`.
//!
//! A thin shell around [`keepster_testkit::tripwire`]: it checks that the machine and the committed
//! thresholds allow a meaningful run, measures, prints the report and writes it to
//! `target/bench/report-<run>.txt`, and exits 0 when no threshold was breached, 1 when one was, and 2
//! when it refused to measure or could not finish. `make` turns 1 and 2 alike into its own 2, so the
//! last line printed says which it was.
//!
//! The fixture is written beneath `KEEPSTER_BENCH_DIR`, or the target directory's temporary directory
//! when that is unset: never `/tmp`, which is in memory on many machines.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use keepster_testkit::bulk::{self, BENCH_PROFILES, Profile};
use keepster_testkit::environment::{self, EnvironmentError};
use keepster_testkit::report::Report;
use keepster_testkit::thresholds::Thresholds;
use keepster_testkit::tripwire::{self, Mode, Repeats};
use keepster_testkit::verdict::{Outcome, Verdict, outcome};

/// The variable naming the directory the fixture is written beneath.
const BENCH_DIR: &str = "KEEPSTER_BENCH_DIR";

/// Why a run ended without verdicts. Both exit 2; what is printed tells them apart.
enum Stop {
    /// It would not measure: the arguments, the machine or the committed thresholds would make its
    /// numbers meaningless.
    Refused(Box<dyn Error>),
    /// It began and could not finish.
    Failed(Box<dyn Error>),
}

/// A [`Stop::Refused`] for `why`.
fn refused(why: impl Into<Box<dyn Error>>) -> Stop {
    Stop::Refused(why.into())
}

/// A [`Stop::Failed`] for `why`.
fn failed(why: impl Into<Box<dyn Error>>) -> Stop {
    Stop::Failed(why.into())
}

/// Run the tripwire the arguments name, and exit with how it ended.
fn main() -> ExitCode {
    let ended = run().unwrap_or_else(|stop| {
        match stop {
            Stop::Refused(why) => eprintln!("tripwire: refused: {}", chain(why.as_ref())),
            Stop::Failed(why) => eprintln!("tripwire: could not finish: {}", chain(why.as_ref())),
        }
        Outcome::Refused
    });
    ExitCode::from(ended.code())
}

/// The run the arguments name, start to finish: how it ended, with its last line printed, or why it
/// stopped.
fn run() -> Result<Outcome, Stop> {
    let mode = Mode::from_args(env::args().skip(1)).map_err(refused)?;
    let base = env::var_os(BENCH_DIR)
        .map_or_else(|| PathBuf::from(env!("CARGO_TARGET_TMPDIR")), PathBuf::from);
    fs::create_dir_all(&base)
        .map_err(|error| refused(format!("cannot create {}: {error}", base.display())))?;
    let environment = environment::detect(&base).map_err(|error| match error {
        EnvironmentError::InMemory { .. } => {
            refused(format!("{error}; set {BENCH_DIR} to a directory on a disk"))
        }
        other => refused(other),
    })?;
    let git = environment::git().map_err(refused)?;
    let fixtures = mode.fixtures();
    let profiles = fixtures.used(mode.scope());
    let digests = profiles
        .iter()
        .map(|profile| digest(profile).map(|digest| (profile.name, digest)))
        .collect::<Result<Vec<_>, _>>()?;

    let thresholds = match mode {
        Mode::Gate => {
            let committed = Thresholds::committed().map_err(refused)?;
            for (profile, digest) in &digests {
                if committed.digest(profile) != Some(digest.as_str()) {
                    return Err(refused(format!(
                        "the {profile} profile is not the one the thresholds were calibrated \
                         against; recalibrate with `make bench-calibrate`"
                    )));
                }
            }
            Some(committed)
        }
        Mode::Calibrate | Mode::Ceiling => None,
    };

    let measured = tripwire::measure(&fixtures, mode.scope(), &base, Repeats::TRIPWIRE, &git)
        .map_err(failed)?;
    let rows = tripwire::rows(&measured, mode, thresholds.as_ref()).map_err(refused)?;
    let named: Vec<(&str, &str)> = digests
        .iter()
        .map(|(name, digest)| (*name, digest.as_str()))
        .collect();
    let report = Report {
        profiles: &named,
        environment: &environment,
        rows: &rows,
        estimates: &tripwire::estimates(&measured, mode.scope()),
    }
    .to_string();
    print!("{report}");
    let written = write_report(mode.name(), &report)?;
    println!("tripwire: report written to {}", written.display());

    let verdicts: Vec<Verdict> = rows.iter().map(|row| row.verdict()).collect();
    let ended = outcome(&verdicts);
    match mode {
        Mode::Gate if ended == Outcome::Breached => {
            println!("tripwire: BREACHED: a gated line measured above its committed limit");
        }
        Mode::Gate => println!("tripwire: passed"),
        Mode::Calibrate => {
            let every = BENCH_PROFILES
                .iter()
                .map(|(profile, contents)| {
                    (profile.name.to_owned(), bulk::digest(profile, *contents))
                })
                .collect();
            let path = Thresholds::committed_path();
            fs::write(&path, tripwire::calibrate(&measured, every).render())
                .map_err(|error| failed(format!("cannot write {}: {error}", path.display())))?;
            println!(
                "tripwire: calibrated; thresholds written to {}, commit them with why they moved",
                path.display()
            );
        }
        Mode::Ceiling => {
            let over = verdicts
                .iter()
                .filter(|verdict| **verdict == Verdict::OverBudget)
                .count();
            println!("tripwire: {over} lines over budget; a ceiling run gates nothing");
        }
    }
    Ok(ended)
}

/// `profile`'s digest, with its contents hashed or not as the thresholds file records it.
///
/// # Errors
///
/// When `profile` is not one the bench measures: a mistake in the bench, never in the machine.
fn digest(profile: &Profile) -> Result<String, Stop> {
    BENCH_PROFILES
        .iter()
        .find(|(benched, _)| benched.name == profile.name)
        .map(|(_, contents)| bulk::digest(profile, *contents))
        .ok_or_else(|| refused(format!("the {} profile is not benched", profile.name)))
}

/// Write `report` to `target/bench/report-<run>.txt`, beside the build that measured it, one file per
/// run so a gate and a calibration never overwrite each other's: where it was written.
fn write_report(run: &str, report: &str) -> Result<PathBuf, Stop> {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .parent()
        .ok_or_else(|| failed("the target's temporary directory has no parent"))?
        .join("bench");
    fs::create_dir_all(&directory)
        .map_err(|error| failed(format!("cannot create {}: {error}", directory.display())))?;
    let path = directory.join(format!("report-{run}.txt"));
    fs::write(&path, report)
        .map_err(|error| failed(format!("cannot write {}: {error}", path.display())))?;
    Ok(path)
}

/// `error` and every error under it, each said once, joined by `: `.
fn chain(error: &dyn Error) -> String {
    let mut said = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        said.push_str(": ");
        said.push_str(&cause.to_string());
        source = cause.source();
    }
    said
}
