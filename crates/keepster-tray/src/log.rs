//! The tray's own log, `tray.log` beside the daemon's (ticket 67): rotated on size within a ceiling of
//! its own, at the level the daemon writes at, so that one `keepster log-level` raise reaches both.

use std::io;
use std::path::Path;
use std::sync::Arc;

use keepster_service::rotating::{self, RotatingFile};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Registry, reload};

/// What the file and its rotated copies hold at most: a tray says little, and a fifth of the daemon's
/// default is plenty for what it does say.
pub const CEILING: u64 = 5 * 1024 * 1024;

/// The log, once installed: the handle that changes its level.
pub struct TrayLog {
    /// Swaps the filter the subscriber writes through.
    levels: reload::Handle<Targets, Registry>,
}

impl TrayLog {
    /// Start writing into `directory` at `level`. Once per process: the subscriber is process-wide.
    pub fn install(directory: &Path, level: &str) -> io::Result<Self> {
        let (size, keep) = rotating::files_for(CEILING);
        let file = Arc::new(RotatingFile::open(
            directory,
            keepster_service::TRAY_LOG_FILE,
            size,
            keep,
        )?);
        let (levels, handle) = reload::Layer::new(targets(level));
        let lines = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(Writer(file));
        tracing::subscriber::set_global_default(Registry::default().with(levels).with(lines))
            .map_err(io::Error::other)?;

        Ok(Self { levels: handle })
    }

    /// Write at `level` from now on.
    pub fn set_level(&self, level: &str) {
        if let Err(error) = self.levels.reload(targets(level)) {
            tracing::warn!(%error, "the tray's log level could not be changed");
        }
    }
}

/// The tray's own records at `level`, everything else at warnings only; an unknown level reads as
/// `info`, as the daemon reads it.
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
