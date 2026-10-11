//! The capture policy of one Root, read from the `[files]` table of the two configuration levels.
//!
//! This is the part of the configuration that decides what a Snapshot holds. Ticket 42 widens it into
//! the whole schema — the per-Root and pattern blocks of the global file, provenance per key, automatic
//! reload and the last known-good — and this reader is where that grows from. Until then both files are
//! read at every capture and every `explain`, so an edit takes effect at the next one.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use keepster_core::{
    CapturePolicy, DEFAULT_MAX_SIZE, IGNORE_FILE, Level, RootPath, Rule, RuleOrigin, RuleSet,
    SizeRule, ignore_file_rules, parse_size,
};
use serde::Deserialize;
use toml::Spanned;

/// The Root's own configuration file, at the top of the watched directory.
pub const ROOT_CONFIG: &str = ".keepster.toml";

/// Why the configuration could not be read. It is never applied in part: falling back to defaults would
/// drop the user's exclude rules and pull into history exactly what they existed to keep out (ticket 04).
#[derive(Debug, thiserror::Error)]
#[error("{path}: {message}")]
pub struct ConfigError {
    /// The file at fault.
    pub path: PathBuf,
    /// What is wrong with it, for a person.
    pub message: String,
}

/// Something about the configuration that is defined but almost certainly not what was meant.
///
/// A warning, never an error: the configuration is well-formed and the daemon's behaviour is fully
/// defined, so the Root does not become `Degraded` over it (ADR 0007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    /// What kind of warning.
    pub code: WarningCode,
    /// What it means here, for a person.
    pub message: String,
}

/// The kinds of configuration warning. Machine-readable, so a client branches on the kind rather than
/// on the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningCode {
    /// A negation an excluded ancestor makes unreachable (ADR 0007).
    NegationUnreachable,
    /// A key a Root's own file may not set, which is therefore ignored (ticket 04).
    KeyIgnored,
}

impl WarningCode {
    /// The code as the wire spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NegationUnreachable => "negation_unreachable",
            Self::KeyIgnored => "key_ignored",
        }
    }
}

/// A Root's capture policy, and what was worth warning about while reading it.
#[derive(Debug, Clone)]
pub struct LoadedPolicy {
    /// What decides what a Snapshot holds.
    pub policy: CapturePolicy,
    /// What was defined but suspicious.
    pub warnings: Vec<ConfigWarning>,
}

#[derive(Debug, Default, Deserialize)]
struct Document {
    // Other tables belong to tickets that have not read them yet; `[files]` itself is strict, so a
    // misspelt key there is refused rather than silently ignored.
    #[serde(default)]
    files: Files,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Files {
    #[serde(default)]
    exclude: Vec<Spanned<String>>,
    #[serde(default)]
    include: Vec<Spanned<String>>,
    respect_gitignore: Option<bool>,
    max_size: Option<Spanned<String>>,
    #[serde(default)]
    max_size_by_pattern: Vec<Spanned<SizeEntry>>,
    #[serde(default)]
    secret_opt_in: Vec<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeEntry {
    pattern: String,
    /// Required, except for a negated pattern, which returns its paths to the scalar cap.
    max: Option<String>,
}

/// One configuration file, read and parsed, with its text kept for line numbers.
struct ConfigFile {
    level: Level,
    path: PathBuf,
    text: String,
    files: Files,
}

/// Read the capture policy of the Root at `root_path`, from the global file at `global` (if any) and the
/// Root's own `.keepster.toml` (if it exists).
pub fn load_policy(global: Option<&Path>, root_path: &Path) -> Result<LoadedPolicy, ConfigError> {
    let mut levels = Vec::new();
    if let Some(global) = global {
        levels.extend(read(global, Level::Global)?);
    }
    levels.extend(read(&root_path.join(ROOT_CONFIG), Level::Root)?);

    let mut warnings = Vec::new();
    let mut rules = Vec::new();
    let mut size_rules = Vec::new();
    let mut opt_in = Vec::new();
    let mut max_size = (DEFAULT_MAX_SIZE, None);
    let mut honour_ignore_files = false;

    for file in &levels {
        let origin = |span: std::ops::Range<usize>| RuleOrigin {
            level: file.level,
            source: file.path.to_string_lossy().into_owned(),
            line: Some(line_of(&file.text, span.start)),
        };
        let invalid = |message: String| ConfigError {
            path: file.path.clone(),
            message,
        };

        // Within a level, `include` comes after `exclude`, so a level's own exceptions win over its own
        // exclude rules; across levels the more specific level wins either way.
        for pattern in &file.files.exclude {
            rules.push(
                Rule::parse(pattern.get_ref(), RootPath::root(), origin(pattern.span()))
                    .map_err(|error| invalid(format!("files.exclude: {error}")))?,
            );
        }
        for pattern in &file.files.include {
            rules.push(
                Rule::include(pattern.get_ref(), RootPath::root(), origin(pattern.span()))
                    .map_err(|error| invalid(format!("files.include: {error}")))?,
            );
        }
        for entry in &file.files.max_size_by_pattern {
            let rule = Rule::parse(
                &entry.get_ref().pattern,
                RootPath::root(),
                origin(entry.span()),
            )
            .map_err(|error| invalid(format!("files.max_size_by_pattern: {error}")))?;
            let max = match (&entry.get_ref().max, rule.negated) {
                (Some(max), _) => parse_size(max)
                    .map_err(|error| invalid(format!("files.max_size_by_pattern: {error}")))?,
                (None, true) => 0,
                (None, false) => {
                    return Err(invalid(format!(
                        "files.max_size_by_pattern: `{}` needs a `max`",
                        entry.get_ref().pattern
                    )));
                }
            };
            size_rules.push(SizeRule { rule, max });
        }
        if let Some(size) = &file.files.max_size {
            let bytes = parse_size(size.get_ref())
                .map_err(|error| invalid(format!("files.max_size: {error}")))?;
            max_size = (bytes, Some(origin(size.span())));
        }
        if let Some(respect) = file.files.respect_gitignore {
            honour_ignore_files = respect;
        }

        if file.level == Level::Root {
            // A file arriving in someone else's repository must not decide what may enter your history
            // (ticket 04). The key is ignored, and said to be, rather than making the file invalid.
            if !file.files.secret_opt_in.is_empty() {
                warnings.push(ConfigWarning {
                    code: WarningCode::KeyIgnored,
                    message: format!(
                        "{}: files.secret_opt_in is ignored in a Root's own file; set it in the global \
                         configuration instead",
                        file.path.display()
                    ),
                });
            }
        } else {
            for pattern in &file.files.secret_opt_in {
                opt_in.push(
                    Rule::parse(pattern.get_ref(), RootPath::root(), origin(pattern.span()))
                        .map_err(|error| invalid(format!("files.secret_opt_in: {error}")))?,
                );
            }
        }
    }

    let mut policy = CapturePolicy::new(
        RuleSet::new(rules),
        RuleSet::new(opt_in),
        max_size.0,
        size_rules,
    )
    .honouring_ignore_files(honour_ignore_files);
    if let Some(origin) = max_size.1 {
        policy = policy.with_max_size_origin(origin);
    }

    for unreachable in policy.rules().unreachable_negations() {
        warnings.push(ConfigWarning {
            code: WarningCode::NegationUnreachable,
            message: format!(
                "`{}` ({}) can never take effect: `{}` ({}) excludes `{}`, and an excluded directory \
                 is never entered",
                unreachable.negation.text,
                describe(&unreachable.negation.origin),
                unreachable.excluded_by.text,
                describe(&unreachable.excluded_by.origin),
                unreachable.ancestor,
            ),
        });
    }

    Ok(LoadedPolicy { policy, warnings })
}

/// `policy` with the ignore files that govern `path` added, when ignore files are honoured.
///
/// What a walk would have read on its way down to `path`: the ignore file of the Root and of each
/// ancestor directory. `explain` needs exactly that view, without walking the whole Root to get it.
pub fn policy_for_path(policy: &CapturePolicy, root_path: &Path, path: &RootPath) -> CapturePolicy {
    if !policy.honours_ignore_files() {
        return policy.clone();
    }

    std::iter::once(RootPath::root())
        .chain(path.ancestors())
        .fold(policy.clone(), |policy, directory| {
            match read_ignore_file(root_path, &directory) {
                rules if rules.is_empty() => policy,
                rules => policy.with_ignore_rules(rules),
            }
        })
}

/// The rules of the ignore file in `directory`, or none if it has none.
///
/// One that cannot be read is passed over, by `explain` as by the walk: it belongs to the directory's
/// own tooling, not to the user's configuration, so it is not ours to refuse a capture over.
pub fn read_ignore_file(root_path: &Path, directory: &RootPath) -> Vec<Rule> {
    let relative = directory.child(IGNORE_FILE);

    match fs::read_to_string(root_path.join(relative.as_str())) {
        Ok(text) => ignore_file_rules(directory, &text, relative.as_str()),
        Err(_) => Vec::new(),
    }
}

/// What is at a path in the Working Tree right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnDisk {
    /// What kind of thing is there, or `None` when nothing is.
    pub kind: Option<OnDiskKind>,
    /// Its size, when it is a regular file.
    pub size: Option<u64>,
}

/// What kind of thing is at a path, as a capture tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDiskKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link, which this build passes over until ticket 49 stores links.
    Symlink,
    /// A FIFO, socket or device, which a capture never reads (ticket 10).
    Special,
}

/// What is at `path` in the Root at `root_path` right now, never following a link.
pub fn observe(root_path: &Path, path: &RootPath) -> OnDisk {
    match fs::symlink_metadata(root_path.join(path.as_str())) {
        Ok(metadata) if metadata.is_dir() => OnDisk {
            kind: Some(OnDiskKind::Directory),
            size: None,
        },
        Ok(metadata) if metadata.is_file() => OnDisk {
            kind: Some(OnDiskKind::File),
            size: Some(metadata.len()),
        },
        Ok(metadata) if metadata.is_symlink() => OnDisk {
            kind: Some(OnDiskKind::Symlink),
            size: None,
        },
        Ok(_) => OnDisk {
            kind: Some(OnDiskKind::Special),
            size: None,
        },
        Err(_) => OnDisk {
            kind: None,
            size: None,
        },
    }
}

/// `level`, and the line where it has one, for a message.
fn describe(origin: &RuleOrigin) -> String {
    match origin.line {
        Some(line) => format!("{}, line {line}", origin.level),
        None => origin.level.to_string(),
    }
}

fn read(path: &Path, level: Level) -> Result<Option<ConfigFile>, ConfigError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ConfigError {
                path: path.to_path_buf(),
                message: format!("it could not be read: {error}"),
            });
        }
    };

    let document: Document = toml::from_str(&text).map_err(|error| ConfigError {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;

    Ok(Some(ConfigFile {
        level,
        path: path.to_path_buf(),
        text,
        files: document.files,
    }))
}

/// The 1-based line holding byte `offset` of `text`.
fn line_of(text: &str, offset: usize) -> u32 {
    let before = &text[..offset.min(text.len())];
    u32::try_from(before.matches('\n').count() + 1).unwrap_or(u32::MAX)
}
