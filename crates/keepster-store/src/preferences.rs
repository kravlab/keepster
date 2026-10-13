//! The web UI's preferences: the one piece of UI state the daemon keeps (ticket 16, ticket 66).
//!
//! They live in a file of their own beside the Roots' directories, so every browser and profile pointed
//! at the daemon sees the same choice. The file is not a Root's directory (its name is no ULID), so the
//! Registry passes over it, and it is replaced atomically like a Registry record.

use std::fs;
use std::io;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::atomic;

/// The file that holds the preferences, in the store base.
const PREFERENCES_FILE: &str = "preferences.toml";

/// Which palette the web UI paints with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Follow the operating system's light or dark setting, live.
    #[default]
    System,
    /// Always the light palette.
    Light,
    /// Always the dark palette.
    Dark,
}

/// The web UI's preferences, every key with its default.
///
/// A key the file does not name reads as its default, so a file written before a key existed still
/// reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// The palette.
    pub theme: Theme,
    /// The longest line, in characters, a diff is still syntax-highlighted with; past it the diff is
    /// plain text, because highlighting such a line stalls the browser.
    pub syntax_line_limit: u32,
    /// Whether the diff wraps long lines rather than scrolling sideways.
    pub wrap_lines: bool,
}

impl Preferences {
    /// The values `syntax_line_limit` may take: a limit of nothing would never highlight, and past ten
    /// million characters the limit no longer protects anything.
    pub const SYNTAX_LINE_LIMITS: RangeInclusive<u32> = 1..=10_000_000;
}

impl Default for Preferences {
    /// The preferences before anything is chosen: the system's palette, highlighting up to 20 000
    /// characters a line (ticket 16), no wrapping.
    fn default() -> Self {
        Self {
            theme: Theme::System,
            syntax_line_limit: 20_000,
            wrap_lines: false,
        }
    }
}

/// Why the preferences could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum PreferencesError {
    /// The file could not be read or written.
    #[error("the preferences at {path} could not be {operation}")]
    Io {
        /// The file involved.
        path: PathBuf,
        /// What was being attempted, for a message a person can act on.
        operation: &'static str,
        /// The underlying failure.
        #[source]
        source: io::Error,
    },
    /// The file holds something that is not preferences this build understands.
    #[error("the preferences at {path} are unreadable: {reason}")]
    Unreadable {
        /// The file involved.
        path: PathBuf,
        /// What is wrong with it.
        reason: String,
    },
}

/// The preferences file of one store base.
#[derive(Debug, Clone)]
pub struct PreferencesStore {
    path: PathBuf,
}

impl PreferencesStore {
    /// The preferences kept in `base`, the directory that holds the Roots' directories. Touches nothing
    /// on disk.
    pub fn at(base: &Path) -> Self {
        Self {
            path: base.join(PREFERENCES_FILE),
        }
    }

    /// The preferences as stored; the defaults when nothing has been written yet.
    ///
    /// # Errors
    ///
    /// [`PreferencesError::Io`] when the file exists but cannot be read, and
    /// [`PreferencesError::Unreadable`] when it holds something other than preferences, such as a theme
    /// this build does not know. Neither is guessed at; the next [`write`](Self::write) replaces the file.
    pub fn read(&self) -> Result<Preferences, PreferencesError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Preferences::default());
            }
            Err(source) => {
                return Err(PreferencesError::Io {
                    path: self.path.clone(),
                    operation: "read",
                    source,
                });
            }
        };

        toml::from_str(&text).map_err(|error| PreferencesError::Unreadable {
            path: self.path.clone(),
            reason: error.message().to_owned(),
        })
    }

    /// Replace the stored preferences atomically: a reader sees the old ones or the new ones.
    ///
    /// # Errors
    ///
    /// [`PreferencesError::Io`] when the file cannot be written, and [`PreferencesError::Unreadable`] in
    /// the unlikely case that the preferences cannot be put into TOML at all, as a Registry record write
    /// reports it.
    pub fn write(&self, preferences: &Preferences) -> Result<(), PreferencesError> {
        let text =
            toml::to_string_pretty(preferences).map_err(|error| PreferencesError::Unreadable {
                path: self.path.clone(),
                reason: error.to_string(),
            })?;

        atomic::replace_file(&self.path, text.as_bytes()).map_err(|source| PreferencesError::Io {
            path: self.path.clone(),
            operation: "written",
            source,
        })
    }
}
