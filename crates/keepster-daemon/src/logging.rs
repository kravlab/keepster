//! The daemon's log (ticket 62): one file per user, rotated on size, at a level that can be raised for
//! a while and returns by itself.
//!
//! The file is written through `tracing` and a writer of our own, `keepster_service::rotating`: no
//! maintained crate rotates on size with a file count without a second level system and a second
//! configuration surface (ticket 33). Only the binary installs it — the subscriber is process-wide — so
//! a daemon a test starts in-process keeps its level as state alone, and writes no file.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use keepster_api::{ErrorCode, Event, LogLevelRequest, LogStatus, Problem};
use keepster_app::config::{DaemonSettings, LOG_LEVELS};
use keepster_service::rotating::{self, RotatingFile};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Layer, Registry, reload};

use crate::DaemonState;
use crate::events::EventBus;
use crate::problem::ProblemResponse;

/// The file's name in the log directory.
pub const LOG_FILE: &str = "daemon.log";

/// The longest a raise may last: a week covers any incident someone is watching, and nothing longer
/// is a raise rather than a configured level.
const LONGEST_RAISE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The daemon's log file, set up by the binary before the daemon starts and installed once the
/// configuration says how to write it.
#[derive(Debug)]
pub struct FileLog {
    directory: PathBuf,
    installed: OnceLock<Installed>,
}

#[derive(Debug)]
struct Installed {
    file: PathBuf,
    levels: reload::Handle<Targets, Registry>,
}

impl FileLog {
    /// A log to be written into `directory`, the platform's per-user log directory.
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            installed: OnceLock::new(),
        }
    }

    /// Start writing: at `settings.log_level`, in `settings.log_format`, within `settings.log_ceiling`
    /// bytes over the file and its rotated copies. Once per process.
    pub(crate) fn install(&self, settings: &DaemonSettings) -> io::Result<PathBuf> {
        let (size, keep) = rotating::files_for(settings.log_ceiling);
        let file = Arc::new(RotatingFile::open(&self.directory, LOG_FILE, size, keep)?);
        let path = file.path();
        let (levels, handle) = reload::Layer::new(targets(&settings.log_level));
        let lines = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(Writer(file));
        let lines = if settings.log_format == "json" {
            lines.json().boxed()
        } else {
            lines.boxed()
        };
        let subscriber = Registry::default().with(levels).with(lines);
        tracing::subscriber::set_global_default(subscriber).map_err(io::Error::other)?;
        let _ = self.installed.set(Installed {
            file: path.clone(),
            levels: handle,
        });

        Ok(path)
    }

    fn set_level(&self, level: &str) {
        if let Some(installed) = self.installed.get() {
            if let Err(error) = installed.levels.reload(targets(level)) {
                eprintln!("keepster: the log level could not be changed: {error}");
            }
        }
    }

    /// The file being written, once it is.
    pub fn file(&self) -> Option<&Path> {
        self.installed
            .get()
            .map(|installed| installed.file.as_path())
    }
}

/// Keepster's own records at `level`; everything else — the HTTP stack, the watcher library — at
/// warnings only, so a raise shows what the daemon did rather than every byte it read.
fn targets(level: &str) -> Targets {
    let level = level.parse::<LevelFilter>().unwrap_or(LevelFilter::INFO);

    Targets::new()
        .with_target("keepster", level)
        .with_default(LevelFilter::WARN.min(level))
}

/// What hands the formatter the log file, one whole line per write.
struct Writer(Arc<RotatingFile>);

impl<'a> MakeWriter<'a> for Writer {
    type Writer = &'a RotatingFile;

    fn make_writer(&'a self) -> Self::Writer {
        &self.0
    }
}

/// The level the daemon writes at: the configured one, or a raise until it ends.
#[derive(Debug)]
pub(crate) struct LogLevels {
    configured: String,
    /// The raise in force and which raise is the latest — one lock over both, and over applying a
    /// level, so a raise and an end arriving together can never leave a level nothing will end.
    state: Mutex<Raises>,
    output: Option<Arc<FileLog>>,
    /// Where a raise beginning or ending is announced, so that a tray takes its level from the stream
    /// (ticket 67).
    events: Arc<EventBus>,
}

#[derive(Debug, Default)]
struct Raises {
    current: Option<Raised>,
    /// Bumped by every raise and end, so an earlier raise's timer never ends a later one.
    generation: u64,
}

#[derive(Debug, Clone)]
struct Raised {
    level: String,
    /// The wall-clock moment it ends. A monotonic clock stops while a laptop sleeps, and the raise
    /// left on overnight is exactly the one on a laptop that slept: the end is the moment, not a delay.
    until: std::time::SystemTime,
}

/// How often a raise's end is checked against the wall clock while it lasts.
const EXPIRY_CHECK: Duration = Duration::from_secs(5);

impl LogLevels {
    /// Levels starting at `configured`, written to `output` when there is a file, and announced on
    /// `events` whenever a raise begins or ends.
    pub(crate) fn new(
        configured: String,
        output: Option<Arc<FileLog>>,
        events: Arc<EventBus>,
    ) -> Self {
        Self {
            configured,
            state: Mutex::new(Raises::default()),
            output,
            events,
        }
    }

    pub(crate) fn status(&self) -> LogStatus {
        let raised = {
            let mut state = self.state();
            self.expire(&mut state);
            state.current.clone()
        };

        LogStatus {
            level: self.level(raised.as_ref()),
            default: self.configured.clone(),
            until: raised.and_then(|raised| crate::roots::rfc3339(raised.until)),
            file: self.file().map(|file| file.display().to_string()),
        }
    }

    /// The file the log is written to, when the daemon writes one.
    pub(crate) fn file(&self) -> Option<PathBuf> {
        self.output
            .as_ref()
            .and_then(|output| output.file().map(Path::to_path_buf))
    }

    /// Write at `level` until `duration` from now, then at the configured level again.
    fn raise(self: &Arc<Self>, level: String, duration: Duration) {
        let until = crate::roots::now() + duration;
        let generation = {
            let mut state = self.state();
            state.generation += 1;
            self.apply(&level);
            tracing::info!(%level, seconds = duration.as_secs(), "the log level is raised");
            state.current = Some(Raised { level, until });
            self.announce(&state);
            state.generation
        };

        let levels = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(EXPIRY_CHECK.min(duration)).await;
                let mut state = levels.state();
                if state.generation != generation {
                    return;
                }
                if levels.expire(&mut state) {
                    return;
                }
            }
        });
    }

    /// Return to the configured level now.
    fn end(&self) {
        let mut state = self.state();
        state.generation += 1;
        self.lower(&mut state);
    }

    /// End the raise in force if its moment has come; whether there is none left.
    fn expire(&self, state: &mut Raises) -> bool {
        match &state.current {
            Some(raised) if crate::roots::now() >= raised.until => {
                self.lower(state);
                true
            }
            Some(_) => false,
            None => true,
        }
    }

    fn lower(&self, state: &mut Raises) {
        if state.current.take().is_some() {
            // Said while still at the raised level, so a quiet configured level cannot hide it.
            tracing::info!(level = %self.configured, "the log level returns to its default");
            self.apply(&self.configured);
            self.announce(state);
        }
    }

    /// The level written at while `raised` is the raise in force.
    fn level(&self, raised: Option<&Raised>) -> String {
        raised.map_or_else(|| self.configured.clone(), |raised| raised.level.clone())
    }

    /// Tell every open event stream the level written at now, and when a raise ends while one lasts.
    fn announce(&self, state: &Raises) {
        self.events.publish_now(Event::LogLevelChanged {
            level: self.level(state.current.as_ref()),
            until: state
                .current
                .as_ref()
                .and_then(|raised| crate::roots::rfc3339(raised.until)),
        });
    }

    fn apply(&self, level: &str) {
        if let Some(output) = &self.output {
            output.set_level(level);
        }
    }

    /// Whether `level` writes more than the configured one: only that is a raise.
    fn raises(&self, level: &str) -> bool {
        let rank = |name: &str| LOG_LEVELS.iter().position(|known| *known == name);
        rank(level) > rank(&self.configured)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, Raises> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Raise the log level for a while.
#[utoipa::path(
    put,
    path = "/v1/log/level",
    operation_id = "raise_log_level",
    tag = "status",
    request_body = LogLevelRequest,
    responses(
        (status = 200, description = "The level is raised until `until`, then returns to `default` by itself; `status` shows it for as long as it lasts. A later raise replaces this one and its end.", body = LogStatus),
        (status = 400, description = "An unknown level, one no more verbose than the configured one, or no duration, or one outside a second to seven days (`log_level_invalid`); or a body that is not this JSON (`body_invalid`).", body = Problem, content_type = "application/problem+json"),
    ),
)]
pub(crate) async fn raise(
    State(state): State<DaemonState>,
    body: Result<Json<LogLevelRequest>, JsonRejection>,
) -> Result<Json<LogStatus>, ProblemResponse> {
    let invalid = |detail: String| {
        ProblemResponse::new(StatusCode::BAD_REQUEST, ErrorCode::LogLevelInvalid, detail)
    };
    let Json(request) = body.map_err(|rejection| {
        ProblemResponse::new(
            rejection.status(),
            ErrorCode::BodyInvalid,
            rejection.body_text(),
        )
    })?;
    if !LOG_LEVELS.contains(&request.level.as_str()) {
        return Err(invalid(format!(
            "`{}` is not a level; write {}",
            request.level,
            LOG_LEVELS.join(", ")
        )));
    }
    if !state.log.raises(&request.level) {
        return Err(invalid(format!(
            "`{}` writes no more than the configured `{}`, so it is not a raise",
            request.level, state.log.configured
        )));
    }
    let Some(written) = request.duration else {
        return Err(invalid(
            "a raise needs `for`, how long it lasts: a raised level always ends".to_owned(),
        ));
    };
    let duration = keepster_core::parse_duration(&written)
        .map_err(|error| invalid(format!("`for`: {error}")))?;
    if duration < Duration::from_secs(1) || duration > LONGEST_RAISE {
        return Err(invalid(format!(
            "`for` is `{written}`; a raise lasts from a second to seven days"
        )));
    }

    state.log.raise(request.level, duration);

    Ok(Json(state.log.status()))
}

/// End a raise of the log level now.
#[utoipa::path(
    delete,
    path = "/v1/log/level",
    operation_id = "end_log_level_raise",
    tag = "status",
    responses(
        (status = 200, description = "The level is the configured one again. Ending no raise is no error.", body = LogStatus),
    ),
)]
pub(crate) async fn end(State(state): State<DaemonState>) -> Json<LogStatus> {
    state.log.end();

    Json(state.log.status())
}
