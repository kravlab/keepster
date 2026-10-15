//! The tripwire's runner (ticket 30): it writes the fixtures, measures every line's workload through the
//! store's and the watcher's public API, picks the bound each line is held to, and computes what
//! calibration commits.
//!
//! The bench binary is a thin shell around this, so the workloads also run in `cargo test` on fixtures
//! small enough for every platform: a bench nobody runs by hand cannot rot unnoticed. Every moment a
//! capture is dated with is fixed rather than read from the clock, as ADR 0012 has it, one step of the
//! declared churn apart.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::io;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use keepster_core::{CapturePolicy, RootId, RootPath, SnapshotId, Trigger, Verify};
use keepster_store::{
    CaptureRequest, CapturedSnapshot, DiffTarget, Git, HistoryStore, PreviewOptions, TimelineQuery,
    preview,
};
use keepster_watch::Watcher;

use crate::bulk::{self, CEILING, Change, EXCLUSION, FLAT, PlannedFile, Profile, SMALL, TreePlan};
use crate::lines::{
    Budget, CAPTURE_FIRST_FLAT_TREE, CAPTURE_FIRST_UNCACHED, CAPTURE_ONE_CHANGED_PATH,
    CAPTURE_READ_TREE, CAPTURE_STEP, LINES, Line, PREVIEW_FIRST_ROW, PREVIEW_WALK, READ_CHRONOLOGY,
    READ_DIFF, READ_VERSION, RECONCILE_EXCLUDED_SUBTREE, RECONCILE_UNCHANGED, STORE_GROWTH, Small,
    WATCH_EXCLUDED_SUBTREE,
};
use crate::report::{EstimateCheck, Row};
use crate::thresholds::{Thresholds, calibrated_limit};
use crate::verdict::{Bound, Summary};

/// The largest file the diff line diffs, in either version: ticket 30's "Diff two versions, file ≤ 1 MiB".
/// The store diffs files up to eight times larger, which the outcome budget does not promise for.
const DIFFED_MOST: u64 = 1024 * 1024;

/// How many times a workload too short to time alone runs within one measurement (see [`batched`]):
/// enough that each measurement lasts ten milliseconds or more on the machine the tripwire was first
/// calibrated on, so one page fault or context switch is not a measurement of its own. Fixed rather than
/// grown until a run lasts long enough, so a limit means the same operation on every machine.
const READ_BATCH: usize = 10;
/// See [`READ_BATCH`]: reading the small fixture's tree takes a few milliseconds. At the ceiling it takes
/// hundreds and is timed one read at a time, so the p95 the estimate is checked on is of single reads.
const TREE_BATCH: usize = 10;
/// See [`READ_BATCH`]: Capture Preview's first row takes microseconds.
const FIRST_ROW_BATCH: usize = 2_000;
/// See [`READ_BATCH`]: a pass over the exclusion fixture's small included tree.
const EXCLUDED_PASS_BATCH: usize = 20;
/// See [`READ_BATCH`]: installing the exclusion fixture's watches.
const WATCH_BATCH: usize = 10;

/// Which run a command asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `make bench`: the small fixture, held to the committed thresholds.
    Gate,
    /// `make bench-calibrate`: the small fixture, measured to write new thresholds.
    Calibrate,
    /// `make bench-ceiling`: the ceiling fixture, compared with the outcome budget, gating nothing.
    Ceiling,
}

impl Mode {
    /// The run `args` name: none, or one of `gate`, `calibrate` and `ceiling`. The `--bench` that
    /// `cargo bench` adds to every bench's arguments is passed over.
    ///
    /// # Errors
    ///
    /// What is wrong with the arguments, to print: an unknown run, or more than one.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut named = args.into_iter().filter(|arg| arg != "--bench");
        let mode = match named.next().as_deref() {
            None | Some("gate") => Self::Gate,
            Some("calibrate") => Self::Calibrate,
            Some("ceiling") => Self::Ceiling,
            Some(other) => {
                return Err(format!(
                    "no run is called `{other}`: gate, calibrate or ceiling"
                ));
            }
        };
        match named.next() {
            Some(extra) => Err(format!("one run at a time, not also `{extra}`")),
            None => Ok(mode),
        }
    }

    /// Its name, as its command passes it and its report is written under.
    pub fn name(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Calibrate => "calibrate",
            Self::Ceiling => "ceiling",
        }
    }

    /// The lines it measures.
    pub fn scope(self) -> Scope {
        match self {
            Self::Gate | Self::Calibrate => Scope::Small,
            Self::Ceiling => Scope::Ceiling,
        }
    }

    /// The fixtures it measures on.
    pub fn fixtures(self) -> Fixtures {
        let main = match self.scope() {
            Scope::Small => SMALL,
            Scope::Ceiling => CEILING,
        };
        Fixtures {
            main,
            flat: FLAT,
            exclusion: EXCLUSION,
        }
    }
}

/// Which lines a run measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every line with a [`Line::small`] use.
    Small,
    /// Every line with a [`Line::ceiling`] budget.
    Ceiling,
}

impl Scope {
    /// Whether it measures `line`.
    fn measures(self, line: &Line) -> bool {
        match self {
            Self::Small => line.small.is_some(),
            Self::Ceiling => line.ceiling.is_some(),
        }
    }
}

/// How often each workload runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeats {
    /// Runs thrown away first, so the page cache and the allocator are as a steady state finds them.
    pub warmups: usize,
    /// Runs measured.
    pub measured: usize,
}

impl Repeats {
    /// One warm-up and five measured runs, the median of which a threshold judges (ticket 30).
    pub const TRIPWIRE: Self = Self {
        warmups: 1,
        measured: 5,
    };
}

/// The profiles a run's fixtures are generated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixtures {
    /// The Root most lines are measured on, and whose change stream the store grows by.
    pub main: Profile,
    /// The flat tree of the lexicographic-order line; the small run's alone.
    pub flat: Profile,
    /// The tree beside a large excluded one, of the exclusion and watch lines; the small run's alone.
    pub exclusion: Profile,
}

impl Fixtures {
    /// The profiles a `scope` run writes fixtures from: the flat and exclusion ones carry only the small
    /// run's invariant lines, so the ceiling writes the main one alone.
    pub fn used(&self, scope: Scope) -> Vec<&Profile> {
        match scope {
            Scope::Small => vec![&self.main, &self.flat, &self.exclusion],
            Scope::Ceiling => vec![&self.main],
        }
    }
}

/// What a line's workload measured.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    /// The line.
    pub line: &'static Line,
    /// One per measured run, in the line's unit.
    pub samples: Vec<f64>,
    /// How many paths the workload walked, where its budget is a rate; `None` elsewhere.
    pub paths: Option<usize>,
}

/// Why a run could not finish.
#[derive(Debug, thiserror::Error)]
pub enum TripwireError {
    /// A workload, or the fixture under it, failed.
    #[error("could not {doing}")]
    Failed {
        /// What was being done.
        doing: String,
        /// Why it failed.
        source: Box<dyn Error + Send + Sync>,
    },
    /// A gated line has no limit in the thresholds file, so it could not fail: the tripwire refuses to
    /// pretend it judged it.
    #[error("the gated line `{0}` has no limit in the thresholds file; recalibrate")]
    Unlimited(String),
    /// A workload ran but did not do what its line claims to measure, so its time says nothing about
    /// that line.
    #[error("the `{line}` workload did not measure what the line claims: {found}")]
    Mismeasured {
        /// The line.
        line: &'static str,
        /// What the workload found instead.
        found: String,
    },
    /// A line the run's scope measures has no measurement: it would be neither judged nor reported.
    #[error("the line `{0}` was not measured")]
    Unmeasured(&'static str),
}

/// A closure turning an error into a [`TripwireError::Failed`] saying what was being done.
fn failed<E: Into<Box<dyn Error + Send + Sync>>>(
    doing: impl Into<String>,
) -> impl FnOnce(E) -> TripwireError {
    let doing = doing.into();
    move |source| TripwireError::Failed {
        doing,
        source: source.into(),
    }
}

/// Measure every line of `scope` on `fixtures`, written beneath `base`, each `repeats` times.
///
/// The fixtures go in a `keepster-bench` directory of `base`, emptied first, so nothing a previous run
/// left behind is measured. Store growth is measured after a Maintenance Pass by `git`, since the
/// outcome line is the steady-state size, and over the whole store, its derived index included. Every
/// workload checks that it did what its line claims — one changed path, an unchanged tree read as
/// unchanged, a diff with lines — and fails the run otherwise. The measurements come back in [`LINES`]'
/// order.
///
/// # Errors
///
/// The first workload, write or read that fails; [`TripwireError::Mismeasured`] when a workload did not
/// measure what its line claims; [`TripwireError::Unmeasured`] when a line of `scope` went unmeasured.
pub fn measure(
    fixtures: &Fixtures,
    scope: Scope,
    base: &Path,
    repeats: Repeats,
    git: &Git,
) -> Result<Vec<Measurement>, TripwireError> {
    let root = base.join("keepster-bench");
    fresh(&root)?;
    let mut clock = Clock::new(fixtures.main.churn.interval);
    let mut recorded = Recorded::default();

    let main = Fixture::write(&fixtures.main, &root.join("main"))?;
    if scope == Scope::Small {
        let first = repeat(repeats, || main.first_capture(&mut clock))?;
        recorded.add(&CAPTURE_FIRST_UNCACHED, first);
    }
    let history = History::grow(&main, &mut clock, &mut recorded)?;
    history.weigh(&main, git, &mut recorded)?;
    history.read(&main, repeats, &mut recorded)?;
    history.read_tree(scope, repeats, &mut recorded)?;
    start_and_preview(&main, repeats, &mut clock, &mut recorded)?;
    if scope == Scope::Small {
        invariants(
            fixtures,
            &root,
            &main,
            history,
            repeats,
            &mut clock,
            &mut recorded,
        )?;
    }

    recorded.into_measurements(scope)
}

/// What a run has measured so far, by line.
#[derive(Debug, Default)]
struct Recorded(HashMap<&'static str, Measurement>);

impl Recorded {
    /// `samples` of `line`, a workload that counts no paths.
    fn add(&mut self, line: &'static Line, samples: Vec<f64>) {
        self.insert(line, samples, None);
    }

    /// `samples` of `line`, a walk of `paths` paths.
    fn add_walk(&mut self, line: &'static Line, samples: Vec<f64>, paths: usize) {
        self.insert(line, samples, Some(paths));
    }

    /// Record `line`'s measurement, replacing none: each line is measured once a run.
    fn insert(&mut self, line: &'static Line, samples: Vec<f64>, paths: Option<usize>) {
        self.0.insert(
            line.name,
            Measurement {
                line,
                samples,
                paths,
            },
        );
    }

    /// The measurement of every line `scope` measures, in [`LINES`]' order.
    ///
    /// # Errors
    ///
    /// [`TripwireError::Unmeasured`] for the first line of `scope` nothing recorded.
    fn into_measurements(mut self, scope: Scope) -> Result<Vec<Measurement>, TripwireError> {
        LINES
            .iter()
            .filter(|line| scope.measures(line))
            .map(|line| {
                self.0
                    .remove(line.name)
                    .ok_or(TripwireError::Unmeasured(line.name))
            })
            .collect()
    }
}

/// The main Root's history: a first Snapshot, then one per step of the declared churn, and what the
/// Working Tree holds after the last of them.
struct History {
    /// The store the history is in.
    store: HistoryStore,
    /// The first Snapshot.
    first: SnapshotId,
    /// The Snapshot of the last step.
    latest: SnapshotId,
    /// The included files after the last step, by path.
    live: BTreeMap<String, PlannedFile>,
    /// The paths some step modified.
    modified: HashSet<String>,
}

impl History {
    /// Capture `main`'s tree, then apply and capture each step of its declared churn, recording each
    /// step's capture as [`CAPTURE_STEP`].
    fn grow(
        main: &Fixture,
        clock: &mut Clock,
        recorded: &mut Recorded,
    ) -> Result<Self, TripwireError> {
        let store = main.store()?;
        let first = main.capture(&store, clock)?.0.id;
        let mut live: BTreeMap<String, PlannedFile> = main
            .plan
            .files
            .iter()
            .map(|file| (file.path.clone(), file.clone()))
            .collect();
        let mut modified: HashSet<String> = HashSet::new();
        let mut steps = Vec::new();
        let mut latest = first;
        for step in bulk::change_stream(&main.profile, &main.plan) {
            bulk::apply(&main.profile, &step, &main.tree)
                .map_err(failed("apply a step of the change stream"))?;
            for change in &step {
                match change {
                    Change::Modified(file) => {
                        modified.insert(file.path.clone());
                        live.insert(file.path.clone(), file.clone());
                    }
                    Change::Created(file) => {
                        live.insert(file.path.clone(), file.clone());
                    }
                    Change::Deleted(path) => {
                        live.remove(path);
                    }
                }
            }
            let (captured, took) = main.capture(&store, clock)?;
            latest = captured.id;
            steps.push(took);
        }
        recorded.add(&CAPTURE_STEP, steps);

        Ok(Self {
            store,
            first,
            latest,
            live,
            modified,
        })
    }

    /// Run a Maintenance Pass with `git`, then record [`STORE_GROWTH`]: the whole store of `main`, its
    /// derived index included, over the content its Working Tree includes.
    fn weigh(
        &self,
        main: &Fixture,
        git: &Git,
        recorded: &mut Recorded,
    ) -> Result<(), TripwireError> {
        self.store
            .maintain(git)
            .map_err(failed("run a Maintenance Pass over the store"))?;
        let included: u64 = self.live.values().map(|file| file.size).sum();
        if included == 0 {
            return Err(missing("an included byte for the store to grow from"));
        }
        let stored = bytes_under(&main.store_directory()).map_err(failed("measure the store"))?;
        if stored == 0 {
            return Err(TripwireError::Mismeasured {
                line: STORE_GROWTH.name,
                found: "a store of no bytes".to_owned(),
            });
        }
        recorded.add(&STORE_GROWTH, vec![ratio(stored, included)]);
        Ok(())
    }

    /// Record the read lines: [`READ_VERSION`] of the largest included file, [`READ_DIFF`] of the
    /// largest file the stream modified within the outcome line's mebibyte, and [`READ_CHRONOLOGY`] of
    /// that file's path.
    fn read(
        &self,
        main: &Fixture,
        repeats: Repeats,
        recorded: &mut Recorded,
    ) -> Result<(), TripwireError> {
        let version = largest(
            self.live
                .values()
                .map(|file| (file.path.as_str(), file.size)),
        )
        .ok_or_else(|| missing("a file to read a version of"))?;
        let planned: HashMap<&str, u64> = main
            .plan
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.size))
            .collect();
        // Both versions within the outcome line's mebibyte: the first as planned, the latest as it is.
        let diffed = largest(
            self.live
                .values()
                .filter(|file| {
                    self.modified.contains(&file.path)
                        && planned
                            .get(file.path.as_str())
                            .is_some_and(|first| (*first).max(file.size) <= DIFFED_MOST)
                })
                .map(|file| (file.path.as_str(), file.size)),
        )
        .ok_or_else(|| {
            missing("a file of at most a mebibyte the change stream modified to diff")
        })?;
        let version = root_path(version)?;
        let diffed = root_path(diffed)?;

        let reads = repeat(repeats, || {
            batched(READ_BATCH, || {
                timed(|| {
                    let content = self
                        .store
                        .content(self.latest, &version)
                        .map_err(failed("find a version"))?;
                    content
                        .read(0, content.size(), &mut |_| Ok(()))
                        .map_err(failed("read a version"))
                })
                .map(|(_, took)| took)
            })
        })?;
        recorded.add(&READ_VERSION, reads);
        let diffs = repeat(repeats, || {
            let (comparison, took) = timed(|| {
                self.store
                    .diff(
                        self.first,
                        &DiffTarget::Snapshot(self.latest),
                        Some(&diffed),
                    )
                    .map_err(failed("diff two versions"))
            })?;
            match comparison.files.as_slice() {
                [file] if !file.too_large && !file.hunks.is_empty() => Ok(took),
                files => Err(TripwireError::Mismeasured {
                    line: READ_DIFF.name,
                    found: format!(
                        "{} files, the first {}",
                        files.len(),
                        files.first().map_or("absent", |file| if file.too_large {
                            "too large"
                        } else {
                            "without a changed line"
                        })
                    ),
                }),
            }
        })?;
        recorded.add(&READ_DIFF, diffs);
        let chronologies = repeat(repeats, || {
            let query = TimelineQuery {
                path: Some(diffed.clone()),
                before: None,
                not_before: None,
                limit: 100,
            };
            batched(READ_BATCH, || {
                timed(|| {
                    self.store
                        .timeline(&query)
                        .map_err(failed("read a chronology"))
                })
                .map(|(_, took)| took)
            })
        })?;
        recorded.add(&READ_CHRONOLOGY, chronologies);
        Ok(())
    }

    /// Record [`CAPTURE_READ_TREE`]: reading the latest Snapshot's tree into an index, in batches at the
    /// small scope and one read at a time at the ceiling.
    fn read_tree(
        &self,
        scope: Scope,
        repeats: Repeats,
        recorded: &mut Recorded,
    ) -> Result<(), TripwireError> {
        // As a capture seeds its index from the previous Snapshot's tree (ADR 0004): an owner-less
        // index, in a repository opened for that read alone. The product opens its store afresh for
        // every pass, so every subtree comes from the object database; a repository kept open would
        // serve them from libgit2's object cache from the second read on. Read after the Maintenance
        // Pass: the store's steady state, its objects packed.
        let seed = latest_commit(&self.store, self.latest)?;
        let batch = match scope {
            Scope::Small => TREE_BATCH,
            Scope::Ceiling => 1,
        };
        let tree_reads = repeat(repeats, || {
            batched(batch, || {
                let repository = git2::Repository::open_bare(self.store.path())
                    .map_err(failed("open the store's repository"))?;
                let tree = repository
                    .find_commit(seed)
                    .and_then(|commit| commit.tree())
                    .map_err(failed("find the latest Snapshot's tree"))?;
                // The index is handed out of the timing, so freeing it — which a capture does only
                // after writing its tree — is not counted.
                let (index, took) = timed(|| {
                    let mut index = git2::Index::new().map_err(failed("make an index"))?;
                    index
                        .read_tree(&tree)
                        .map_err(failed("read a Snapshot's tree"))?;
                    Ok(index)
                })?;
                if index.len() < self.live.len() {
                    return Err(TripwireError::Mismeasured {
                        line: CAPTURE_READ_TREE.name,
                        found: format!(
                            "{} entries for {} files in the Working Tree",
                            index.len(),
                            self.live.len()
                        ),
                    });
                }
                Ok(took)
            })
        })?;
        recorded.add(&CAPTURE_READ_TREE, tree_reads);
        Ok(())
    }
}

/// Record the lines of a daemon's start and of registration over `main`'s unchanged tree:
/// [`RECONCILE_UNCHANGED`], [`PREVIEW_FIRST_ROW`] and [`PREVIEW_WALK`].
fn start_and_preview(
    main: &Fixture,
    repeats: Repeats,
    clock: &mut Clock,
    recorded: &mut Recorded,
) -> Result<(), TripwireError> {
    let startups = repeat(repeats, || main.start_up(clock))?;
    recorded.add(&RECONCILE_UNCHANGED, startups);

    let first_rows = repeat(repeats, || {
        batched(FIRST_ROW_BATCH, || main.preview_first_row())
    })?;
    recorded.add(&PREVIEW_FIRST_ROW, first_rows);
    let mut walked = 0;
    let walks = repeat(repeats, || {
        let (paths, took) = main.preview_walk()?;
        walked = paths;
        Ok(took)
    })?;
    recorded.add_walk(&PREVIEW_WALK, walks, walked);
    Ok(())
}

/// Record the small run's invariant lines: [`CAPTURE_ONE_CHANGED_PATH`] on `main`'s `history`, the two
/// exclusion lines on the exclusion fixture, and [`CAPTURE_FIRST_FLAT_TREE`] on the flat one.
fn invariants(
    fixtures: &Fixtures,
    root: &Path,
    main: &Fixture,
    history: History,
    repeats: Repeats,
    clock: &mut Clock,
    recorded: &mut Recorded,
) -> Result<(), TripwireError> {
    let candidates: Vec<PlannedFile> = history.live.into_values().collect();
    if candidates.is_empty() {
        return Err(missing("a file to change"));
    }
    let mut changed = 0;
    let one_changed = repeat(repeats, || {
        changed += 1;
        main.rewrite(&candidates[changed % candidates.len()], changed)?;
        let (captured, took) = main.capture(&history.store, clock)?;
        match captured.changed_paths {
            Ok(1) => Ok(took),
            counted => Err(TripwireError::Mismeasured {
                line: CAPTURE_ONE_CHANGED_PATH.name,
                found: format!("{counted:?} changed paths"),
            }),
        }
    })?;
    recorded.add(&CAPTURE_ONE_CHANGED_PATH, one_changed);

    let exclusion = Fixture::write(&fixtures.exclusion, &root.join("exclusion"))?;
    let excluded_store = exclusion.store()?;
    exclusion.capture(&excluded_store, clock)?;
    let passes = repeat(repeats, || {
        batched(EXCLUDED_PASS_BATCH, || {
            exclusion.pass(&excluded_store, clock, &RECONCILE_EXCLUDED_SUBTREE)
        })
    })?;
    recorded.add(&RECONCILE_EXCLUDED_SUBTREE, passes);
    let watches = repeat(repeats, || batched(WATCH_BATCH, || exclusion.watch()))?;
    recorded.add(&WATCH_EXCLUDED_SUBTREE, watches);

    let flat = Fixture::write(&fixtures.flat, &root.join("flat"))?;
    let flat_firsts = repeat(repeats, || flat.first_capture(clock))?;
    recorded.add(&CAPTURE_FIRST_FLAT_TREE, flat_firsts);
    Ok(())
}

/// The rows a `mode` run reports: each measurement held to the bound that run holds its line to — a
/// gated line's committed limit at the gate, ticket 30's budget at the ceiling, and nothing when
/// calibrating, which measures in order to set the limits. A line held to an estimate is recorded, and
/// [`estimates`] checks it. At the gate a baseline says why its line is not gated, as ticket 87's line
/// says that the invariant is not in the code yet. Only the gate reads `thresholds`, the committed ones.
///
/// # Errors
///
/// [`TripwireError::Unlimited`] at the gate, for a gated line the thresholds, if any, hold no limit for;
/// [`TripwireError::Mismeasured`] at the ceiling, for a line held to a rate that counted no paths.
pub fn rows(
    measured: &[Measurement],
    mode: Mode,
    thresholds: Option<&Thresholds>,
) -> Result<Vec<Row>, TripwireError> {
    measured
        .iter()
        .map(|measurement| {
            let line = measurement.line;
            let row = |bound| Row::new(line.name, line.unit, &measurement.samples, bound);
            Ok(match mode {
                Mode::Calibrate => row(Bound::Baseline),
                Mode::Gate => match line.small {
                    Some(Small::Baseline { why }) => row(Bound::Baseline).because(why),
                    None => row(Bound::Baseline),
                    Some(Small::Gated) => row(Bound::Threshold(
                        thresholds
                            .and_then(|thresholds| thresholds.limit(line.name))
                            .ok_or_else(|| TripwireError::Unlimited(line.name.to_owned()))?,
                    )),
                },
                Mode::Ceiling => match line.ceiling {
                    Some(Budget::Estimate { .. }) | None => row(Bound::Baseline),
                    Some(budget) => row(Bound::Budget(budget.most(measurement.paths).ok_or_else(
                        || TripwireError::Mismeasured {
                            line: line.name,
                            found: "a rate without the paths it walked".to_owned(),
                        },
                    )?)),
                },
            })
        })
        .collect()
}

/// The check of every line `measured` held to an estimate, in the order measured: ticket 30's
/// comparison of `read_tree` with ticket 12's estimate and with the capture budget it is a part of. An
/// estimate is stated for the ceiling's fixture, so a `scope` other than the ceiling checks none.
///
/// # Panics
///
/// When the capture line has no budget of its own: [`CAPTURE_STEP`] is constant, and its tests hold it
/// to one.
pub fn estimates(measured: &[Measurement], scope: Scope) -> Vec<EstimateCheck> {
    if scope != Scope::Ceiling {
        return Vec::new();
    }
    let Some(Budget::Most(capture_budget)) = CAPTURE_STEP.ceiling else {
        panic!("the capture line has a budget of its own");
    };
    measured
        .iter()
        .filter_map(|measurement| match measurement.line.ceiling {
            Some(Budget::Estimate { most, source }) => Some(EstimateCheck {
                line: measurement.line.name,
                p95: Summary::of(&measurement.samples).p95(),
                most,
                source,
                capture_budget,
            }),
            _ => None,
        })
        .collect()
}

/// The thresholds a calibration commits: [`calibrated_limit`] of each gated line's median, and
/// `digests`, the profiles' as they are now.
pub fn calibrate(measured: &[Measurement], digests: BTreeMap<String, String>) -> Thresholds {
    let limits = measured
        .iter()
        .filter(|measurement| measurement.line.small == Some(Small::Gated))
        .map(|measurement| {
            (
                measurement.line.name.to_owned(),
                calibrated_limit(Summary::of(&measurement.samples).median()),
            )
        })
        .collect();
    Thresholds::calibrated(digests, limits)
}

/// A profile's tree, written, with where its stores go.
struct Fixture {
    /// The profile it was written from.
    profile: Profile,
    /// What was written.
    plan: TreePlan,
    /// What its captures admit.
    policy: CapturePolicy,
    /// The Root's identity in its stores.
    id: RootId,
    /// Where the fixture's directories are.
    directory: PathBuf,
    /// The Root.
    tree: PathBuf,
}

impl Fixture {
    /// Write `profile`'s tree beneath `directory`.
    fn write(profile: &Profile, directory: &Path) -> Result<Self, TripwireError> {
        let plan = bulk::plan(profile);
        let tree = directory.join("tree");
        fs::create_dir_all(&tree).map_err(failed(format!("create {}", tree.display())))?;
        bulk::materialize(profile, &plan, &tree)
            .map_err(failed(format!("write the {} fixture", profile.name)))?;

        Ok(Self {
            profile: *profile,
            policy: profile.policy(),
            plan,
            id: RootId::generate(Clock::start()),
            directory: directory.to_owned(),
            tree,
        })
    }

    /// The fixture's store, opened — and created, the first time.
    fn store(&self) -> Result<HistoryStore, TripwireError> {
        open(&self.store_directory())
    }

    /// Where the fixture's store lies: the git store and the derived index beside it, both of which
    /// take the user's disk.
    fn store_directory(&self) -> PathBuf {
        self.directory.join("store")
    }

    /// Capture the tree into `store`: what was captured, and how long it took.
    fn capture(
        &self,
        store: &HistoryStore,
        clock: &mut Clock,
    ) -> Result<(CapturedSnapshot, f64), TripwireError> {
        let now = clock.tick();
        timed(|| {
            store
                .capture(self.id, &self.tree, &self.policy, Trigger::Manual, now)
                .map_err(failed("capture"))
        })
    }

    /// How long a first capture into an empty store takes: with no previous tree to seed from and an
    /// index nothing has cached. The store is made fresh each time and is not timed being made.
    fn first_capture(&self, clock: &mut Clock) -> Result<f64, TripwireError> {
        let directory = self.directory.join("first");
        fresh(&directory)?;
        let store = open(&directory)?;
        self.capture(&store, clock).map(|(_, took)| took)
    }

    /// How long a Reconciliation pass over the unchanged tree takes: one that reads only what the `stat`
    /// filter cannot vouch for and writes no Snapshot.
    ///
    /// # Errors
    ///
    /// [`TripwireError::Mismeasured`], naming `line`, when the pass read the tree as changed: hashed a
    /// path or wrote a Snapshot.
    fn pass(
        &self,
        store: &HistoryStore,
        clock: &mut Clock,
        line: &'static Line,
    ) -> Result<f64, TripwireError> {
        let request = CaptureRequest {
            trigger: Trigger::Reconcile,
            now: clock.tick(),
            verify: Verify::Stat,
            record_unchanged: false,
            hold_emptied: false,
        };
        let (pass, took) = timed(|| {
            store
                .pass(self.id, &self.tree, &self.policy, &request)
                .map_err(failed("reconcile"))
        })?;
        if pass.snapshot.is_some() || pass.hashed > 0 {
            return Err(TripwireError::Mismeasured {
                line: line.name,
                found: format!(
                    "an unchanged tree read as changed: {} paths hashed, {} Snapshot written",
                    pass.hashed,
                    if pass.snapshot.is_some() { "a" } else { "no" }
                ),
            });
        }
        Ok(took)
    }

    /// How long a daemon's start over this Root's unchanged tree takes, as ticket 30's Startup
    /// Reconciliation counts it: the store recovered, a Reconciliation pass, and the watches installed.
    /// The watcher is dropped untimed.
    fn start_up(&self, clock: &mut Clock) -> Result<f64, TripwireError> {
        let directory = self.store_directory();
        let (store, recovered) = timed(|| {
            HistoryStore::recover(
                directory.join("history.git"),
                directory.join("index.sqlite"),
            )
            .map_err(failed(format!(
                "recover the store at {}",
                directory.display()
            )))?
            .ok_or_else(|| missing("a store to recover"))
        })?;
        let passed = self.pass(&store, clock, &RECONCILE_UNCHANGED)?;
        let watched = self.watch()?;
        Ok(recovered + passed + watched)
    }

    /// How long Capture Preview takes to hand over its first row.
    fn preview_first_row(&self) -> Result<f64, TripwireError> {
        let start = Instant::now();
        let mut first = None;
        self.preview(&mut |_| {
            first = Some(start.elapsed());
            ControlFlow::Break(())
        })?;
        first
            .map(milliseconds)
            .ok_or_else(|| missing("a first row of the preview"))
    }

    /// How many paths Capture Preview walks over the whole tree, and how long that takes.
    fn preview_walk(&self) -> Result<(usize, f64), TripwireError> {
        let mut paths = 0;
        timed(|| {
            self.preview(&mut |_| {
                paths += 1;
                ControlFlow::Continue(())
            })
        })
        .map(|((), took)| (paths, took))
    }

    /// Preview the whole tree, as registration does, never entering an excluded directory.
    fn preview(
        &self,
        visit: &mut dyn FnMut(keepster_core::PreviewEntry) -> ControlFlow<()>,
    ) -> Result<(), TripwireError> {
        preview(
            &self.tree,
            &RootPath::root(),
            &self.policy,
            &PreviewOptions {
                measure_excluded: false,
            },
            visit,
        )
        .map_err(failed("preview the tree"))
    }

    /// How long installing the watches takes. The watcher is dropped untimed.
    fn watch(&self) -> Result<f64, TripwireError> {
        let (watcher, took) = timed(|| {
            let placeholder =
                |_: &Path, metadata: &fs::Metadata| keepster_core::is_cloud_placeholder(metadata);
            Watcher::start(&self.tree, &self.policy, placeholder, |_| {})
                .map_err(failed("watch the tree"))
        })?;
        drop(watcher);
        Ok(took)
    }

    /// Write `file` again, `generation` bytes longer and from another seed: a size no earlier write
    /// gave it, so the change is plain to the `stat` filter even where mtimes are whole seconds and the
    /// rewrite falls in the second of the write before it.
    fn rewrite(&self, file: &PlannedFile, generation: usize) -> Result<(), TripwireError> {
        let generation = generation as u64;
        let changed = PlannedFile {
            size: file.size + generation,
            content_seed: file.content_seed.wrapping_add(generation),
            ..file.clone()
        };
        fs::write(
            self.tree.join(&file.path),
            bulk::content(&self.profile, &changed),
        )
        .map_err(failed(format!("rewrite {}", file.path)))
    }
}

/// The moments captures are dated with: fixed, one step of the main fixture's declared churn apart, and
/// far enough ahead of any file's real modification time that none is racily clean.
#[derive(Debug)]
struct Clock {
    /// How far apart the moments are.
    interval: Duration,
    /// Moments handed out so far.
    ticks: u32,
}

impl Clock {
    /// The first moment, 2030-01-01T00:00:00Z, as time since the epoch.
    const START: Duration = Duration::from_secs(1_893_456_000);

    /// A clock whose moments are `interval` apart.
    fn new(interval: Duration) -> Self {
        Self { interval, ticks: 0 }
    }

    /// The first moment, the one every fixture's Root is identified at.
    fn start() -> SystemTime {
        SystemTime::UNIX_EPOCH + Self::START
    }

    /// The next moment.
    fn tick(&mut self) -> SystemTime {
        self.ticks += 1;
        Self::start() + self.interval * self.ticks
    }
}

/// The commit the Snapshot `latest` is in, found as the store reports its own shape: the tip of the
/// Segment whose newest Snapshot it is.
///
/// # Errors
///
/// [`TripwireError::Mismeasured`] when no Segment ends at `latest`: a tree read from another would not
/// be the one a capture seeds from.
fn latest_commit(store: &HistoryStore, latest: SnapshotId) -> Result<git2::Oid, TripwireError> {
    let topology = store.topology().map_err(failed("read the store's shape"))?;
    let segment = topology
        .segments
        .iter()
        .find(|segment| segment.newest == Some(latest))
        .ok_or_else(|| TripwireError::Mismeasured {
            line: CAPTURE_READ_TREE.name,
            found: "no Segment ends at the latest Snapshot".to_owned(),
        })?;
    git2::Oid::from_str(&segment.tip).map_err(failed("name the latest Snapshot's commit"))
}

/// `directory`, emptied of anything a previous run left in it.
fn fresh(directory: &Path) -> Result<(), TripwireError> {
    match fs::remove_dir_all(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(failed(format!("empty {}", directory.display()))(error)),
    }
    fs::create_dir_all(directory).map_err(failed(format!("create {}", directory.display())))
}

/// The store at `directory`, its derived index beside it.
fn open(directory: &Path) -> Result<HistoryStore, TripwireError> {
    HistoryStore::open(
        directory.join("history.git"),
        directory.join("index.sqlite"),
    )
    .map_err(failed(format!("open the store at {}", directory.display())))
}

/// One measurement of a workload too short to time alone: the mean of `batch` runs of `once`, each
/// timing itself, so whatever a run does untimed — dropping a watcher — stays out of it.
fn batched(
    batch: usize,
    mut once: impl FnMut() -> Result<f64, TripwireError>,
) -> Result<f64, TripwireError> {
    let mut total = 0.0;
    for _ in 0..batch {
        total += once()?;
    }
    Ok(total / batch as f64)
}

/// Run `once` `repeats.warmups` times unrecorded, then `repeats.measured` times, recording each.
fn repeat(
    repeats: Repeats,
    mut once: impl FnMut() -> Result<f64, TripwireError>,
) -> Result<Vec<f64>, TripwireError> {
    for _ in 0..repeats.warmups {
        once()?;
    }
    (0..repeats.measured).map(|_| once()).collect()
}

/// What `work` returns, and how many milliseconds it took.
fn timed<T>(work: impl FnOnce() -> Result<T, TripwireError>) -> Result<(T, f64), TripwireError> {
    let start = Instant::now();
    let done = work()?;
    Ok((done, milliseconds(start.elapsed())))
}

/// `elapsed` in milliseconds.
fn milliseconds(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1_000.0
}

/// The path of the largest of `files`, the first by name of equally large ones.
fn largest<'a>(files: impl Iterator<Item = (&'a str, u64)>) -> Option<&'a str> {
    files
        .fold(None, |best: Option<(&str, u64)>, (path, size)| match best {
            Some((_, most)) if most >= size => best,
            _ => Some((path, size)),
        })
        .map(|(path, _)| path)
}

/// `path` as a Root path.
fn root_path(path: &str) -> Result<RootPath, TripwireError> {
    RootPath::parse(path).map_err(failed(format!("name {path} as a Root path")))
}

/// The failure of a fixture that lacks `what` a workload needs.
fn missing(what: &str) -> TripwireError {
    failed(format!("find {what} in the fixture"))(io::Error::from(io::ErrorKind::NotFound))
}

/// Every byte of every file under `directory`.
///
/// Unlike the Maintenance Pass's own count, which reads what it can, a path it cannot read is an error:
/// a store measured short would pass the growth line by being unreadable.
fn bytes_under(directory: &Path) -> io::Result<u64> {
    let mut total = 0;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        total += if entry.file_type()?.is_dir() {
            bytes_under(&entry.path())?
        } else {
            entry.metadata()?.len()
        };
    }
    Ok(total)
}

/// `part` over `whole`, as the ratio a report prints.
fn ratio(part: u64, whole: u64) -> f64 {
    // Byte counts are far below where a float stops counting exactly enough for a ratio to two decimals.
    part as f64 / whole as f64
}
