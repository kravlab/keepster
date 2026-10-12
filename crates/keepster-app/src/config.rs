//! The configuration: two TOML levels, merged per key with the level that set each value, reloaded
//! whenever a file changes, and never applied while invalid.
//!
//! The global file is the user's own; besides its `[files]` table it holds `[root.<RootId>]` and
//! `[[roots]]` blocks that accept the whole schema, so a Root's own `.keepster.toml` is optional — the
//! way to share rules through a repository, not the way to configure a Root (ticket 04). Levels, from
//! the least specific: `default`, `global`, `global-pattern`, `global-root`, `root`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;

use std::time::{Duration, UNIX_EPOCH};

use keepster_core::{
    CapturePolicy, DEFAULT_MAX_SIZE, IGNORE_FILE, InvalidPattern, Keep, Ladder, Level, ModeChoice,
    PathGlob, RootId, RootPath, Rule, RuleOrigin, RuleSet, SizeRule, Trigger, Triggers, Verify,
    WithheldReason, ignore_file_rules, parse_duration, parse_size,
};
use serde::Deserialize;
use toml::Spanned;

/// The Root's own configuration file, at the top of the watched directory.
pub const ROOT_CONFIG: &str = ".keepster.toml";

/// Why the configuration could not be read. It is never applied in part: falling back to defaults would
/// drop the user's exclude rules and pull into history exactly what they existed to keep out (ticket 04).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
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

/// One key of the schema, and the class it belongs to.
///
/// The class decides what may set the key and what changing it does, which is why it is data rather than
/// a rule written into each place that needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    /// The key as a person writes it, `table.key`.
    pub name: &'static str,
    /// Whether a Root's own `.keepster.toml` may set it. Three classes may not, whatever they describe:
    /// keys describing the daemon, keys making it execute something, and keys deciding where the user's
    /// data goes or what may enter it — a file arriving in someone else's repository must not choose
    /// any of them (ticket 04).
    pub root_settable: bool,
    /// Whether an edit takes effect without a restart. One that cannot is reported as restart-required
    /// rather than silently ignored; the first such key is ticket 60's loopback listener.
    pub live: bool,
    /// Whether it describes the daemon rather than a Root: one value per process, so only the global
    /// file's own table sets it, and a per-Root block setting it is ignored with a warning as a Root's
    /// own file is (ticket 60).
    pub daemon: bool,
    /// Whether it decides what a capture holds, so that changing it queues the Root for a full
    /// comparison against its Working Tree.
    pub classifies: bool,
}

/// Every key this build understands, in the order the configuration resource lists them.
///
/// The schema is closed: a key that is not here is refused rather than ignored, because a misspelt
/// `exlude` would otherwise capture what it meant to keep out.
pub const SCHEMA: &[KeySpec] = &[
    capture_key("files.exclude"),
    capture_key("files.include"),
    capture_key("files.respect_gitignore"),
    capture_key("files.max_size"),
    capture_key("files.max_size_by_pattern"),
    // What may enter the user's history past the secret denylist is theirs to decide alone.
    KeySpec {
        name: "files.secret_opt_in",
        root_settable: false,
        live: true,
        classifies: true,
        daemon: false,
    },
    // Following a link decides what may enter the user's history — a file arriving in someone else's
    // repository must not be able to pull `~/.ssh` in through a link of its own (ticket 49).
    KeySpec {
        name: "files.follow_symlinks",
        root_settable: false,
        live: true,
        classifies: true,
        daemon: false,
    },
    trigger_key("files.follow_reconcile_interval"),
    trigger_key("triggers.active"),
    trigger_key("triggers.debounce"),
    trigger_key("triggers.rate_limit"),
    trigger_key("triggers.periodic"),
    trigger_key("reconcile.at_start"),
    trigger_key("reconcile.verify"),
    // Which permission bits a Restore leaves where they diverge, when the request does not say: a
    // default for a question, which the Root's own file may answer like any other preference.
    KeySpec {
        name: "restore.modes",
        root_settable: true,
        live: true,
        classifies: false,
        daemon: false,
    },
    // How much history is kept is a decision about the user's data: a file arriving in someone else's
    // repository must not be able to shorten their history to an hour (ticket 51).
    KeySpec {
        name: "retention.tier",
        root_settable: false,
        live: true,
        classifies: false,
        daemon: false,
    },
    // The loopback listener, its port, and the browser origins it answers besides its own: the one
    // network surface the product has, which a CLI-only user may decline (ticket 24). Taken when the
    // daemon starts, because a listener appearing while it runs would be a second lifecycle.
    daemon_key("ui.listener"),
    daemon_key("ui.port"),
    daemon_key("ui.allowed_origins"),
    // The log describes the daemon too: one file, one format, one budget, whichever Roots there are
    // (ticket 62). The level here is the one the daemon returns to after a raise expires.
    daemon_key("log.level"),
    daemon_key("log.format"),
    daemon_key("log.ceiling"),
];

/// A key describing the daemon: set in the global file's own table alone, and in force from the start
/// until the next one.
const fn daemon_key(name: &'static str) -> KeySpec {
    KeySpec {
        name,
        root_settable: false,
        live: false,
        classifies: false,
        daemon: true,
    }
}

/// The port the loopback listener binds unless `ui.port` says otherwise.
pub const DEFAULT_PORT: u16 = 7419;

/// How much the log's files may hold together unless `log.ceiling` says otherwise (ticket 33).
pub const DEFAULT_LOG_CEILING: u64 = 50 * 1024 * 1024;

/// The smallest ceiling accepted: below it each of the log's files would hold too few lines to read.
const MINIMUM_LOG_CEILING: u64 = 64 * 1024;

/// The levels a log can be at, from the quietest.
pub const LOG_LEVELS: &[&str] = &["error", "warn", "info", "debug", "trace"];

/// The formats a log line can be written in: `key=value` text for a person, or one JSON object.
pub const LOG_FORMATS: &[&str] = &["text", "json"];

/// A key deciding when and how a Root is captured rather than what a capture holds: settable per Root,
/// as ticket 04 lists the Trigger set, its intervals and Reconciliation, and changing it reclassifies
/// nothing.
const fn trigger_key(name: &'static str) -> KeySpec {
    KeySpec {
        name,
        root_settable: true,
        live: true,
        classifies: false,
        daemon: false,
    }
}

const fn capture_key(name: &'static str) -> KeySpec {
    KeySpec {
        name,
        root_settable: true,
        live: true,
        classifies: true,
        daemon: false,
    }
}

/// The schema's entry for `name`, which must be in it.
fn spec(name: &str) -> &'static KeySpec {
    SCHEMA
        .iter()
        .find(|spec| spec.name == name)
        .expect("every key the merge reports is in the schema")
}

/// How often the periodic floor captures once it is switched on, unless configured otherwise.
const DEFAULT_PERIODIC: Duration = Duration::from_secs(3600);

/// How often a Root that follows links passes over their targets, unless configured otherwise.
const DEFAULT_FOLLOW_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// One value, as the schema types it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// A switch.
    Bool(bool),
    /// A size, in bytes.
    Bytes(u64),
    /// A plain number, such as a port.
    Number(u64),
    /// A length of time.
    Duration(Duration),
    /// A set of names, replaced wholesale by a more specific level.
    Words(Vec<String>),
    /// One name out of a closed set.
    Name(String),
    /// The Retention ladder.
    Ladder(Ladder),
    /// A pattern, exactly as written.
    Pattern(String),
    /// A pattern with the size cap it sets; a negated pattern sets none, returning its paths to the
    /// scalar cap.
    SizePattern {
        /// The pattern, exactly as written.
        pattern: String,
        /// The cap in bytes, absent for a negated pattern.
        max: Option<u64>,
    },
}

/// A value, and where it was set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The value.
    pub value: Value,
    /// The level, file and line that set it.
    pub origin: RuleOrigin,
}

/// What a key is set to once every level has been merged.
///
/// The two shapes are the two merge classes: a pattern list layers, every level contributing its
/// entries after the less specific ones; a scalar is replaced wholesale by the most specific level
/// that sets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Setting {
    /// The value in force, and the level that set it — the built-in default when none did.
    Scalar(Entry),
    /// Every entry of a layered list, from the least specific level to the most.
    List(Vec<Entry>),
}

impl Setting {
    /// Where this setting was decided: a scalar's own origin, or a list's last entry's — the most
    /// specific level it has an entry from — and the built-in default for an empty list.
    pub fn origin(&self) -> RuleOrigin {
        match self {
            Self::Scalar(entry) => entry.origin.clone(),
            Self::List(entries) => entries
                .last()
                .map(|entry| entry.origin.clone())
                .unwrap_or_else(default_origin),
        }
    }

    /// The setting without its provenance: what a capture acts on.
    pub fn values(&self) -> Vec<&Value> {
        match self {
            Self::Scalar(entry) => vec![&entry.value],
            Self::List(entries) => entries.iter().map(|entry| &entry.value).collect(),
        }
    }
}

/// How a Root's Working Tree is compared against its latest Snapshot (ticket 44).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconciliation {
    /// Whether a pass runs when the daemon starts. Switching it off drops only that walk — the answer
    /// to a platform's lost events is never optional — and the Root is reported as not guaranteed.
    pub at_start: bool,
    /// How a pass decides a file is unchanged without reading it.
    pub verify: Verify,
    /// How often a Root that cannot be watched is reconciled instead: the periodic floor's interval,
    /// whether or not the floor itself is active.
    pub fallback_every: Duration,
}

/// One key of a Root's effective configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveKey {
    /// Which key, and its class.
    pub spec: &'static KeySpec,
    /// What it is set to, and by which level.
    pub setting: Setting,
    /// Whether a file sets it to something other than the value in force, which only a restart applies.
    pub restart_required: bool,
}

/// One Root's configuration with every level merged.
#[derive(Debug, Clone)]
pub struct Effective {
    /// What decides what a Snapshot holds.
    pub policy: CapturePolicy,
    /// When the Root is captured.
    pub triggers: Triggers,
    /// How the Root is reconciled.
    pub reconcile: Reconciliation,
    /// Which permission bits a Restore leaves where the Snapshot's and the disk's differ and the
    /// request does not say; `None` asks (ticket 50).
    pub restore_modes: Option<ModeChoice>,
    /// Which Snapshots survive (ticket 51).
    pub retention: Ladder,
    /// Every key of the schema, in schema order, with its provenance.
    pub keys: Vec<EffectiveKey>,
    /// What was defined but almost certainly not meant.
    pub warnings: Vec<ConfigWarning>,
}

impl Effective {
    /// The values a capture classifies paths by, without where they were written: a rule moved to
    /// another line changes nothing a capture does.
    fn classification_inputs(&self) -> Vec<(&'static str, Vec<&Value>)> {
        self.keys
            .iter()
            .filter(|key| key.spec.classifies)
            .map(|key| (key.spec.name, key.setting.values()))
            .collect()
    }
}

/// What a Root's own `.keepster.toml` sets in `effective`, key by key in schema order, with the values it
/// wrote — a list's own entries alone: what a Capture Preview shows, so that a rule a repository's author
/// wrote is never read as Keepster's own default.
pub fn set_by_root_file(effective: &Effective) -> Vec<(&'static str, Vec<&Value>)> {
    effective
        .keys
        .iter()
        .filter_map(|key| {
            let entries: Vec<&Entry> = match &key.setting {
                Setting::Scalar(entry) => vec![entry],
                Setting::List(entries) => entries.iter().collect(),
            };
            let written: Vec<&Value> = entries
                .into_iter()
                .filter(|entry| entry.origin.level == Level::Root)
                .map(|entry| &entry.value)
                .collect();
            (!written.is_empty()).then_some((key.spec.name, written))
        })
        .collect()
}

/// `effective` with what Forget excluded added, as rules of its own level and as entries of
/// `files.exclude`, so that `config` shows them where they decide.
fn forgetting(effective: &mut Effective, forgotten: Vec<Rule>) {
    if forgotten.is_empty() {
        return;
    }
    if let Some(exclude) = effective
        .keys
        .iter_mut()
        .find(|key| key.spec.name == "files.exclude")
    {
        if let Setting::List(entries) = &mut exclude.setting {
            entries.extend(forgotten.iter().map(|rule| Entry {
                value: Value::Pattern(rule.text.clone()),
                origin: rule.origin.clone(),
            }));
        }
    }
    effective.policy = std::mem::take(&mut effective.policy).with_forgotten(forgotten);
}

/// The settings one level contributes, parsed and checked, so that merging can never fail.
#[derive(Debug, Clone, Default)]
struct Layer {
    exclude: Vec<Rule>,
    include: Vec<Rule>,
    respect_gitignore: Option<Entry>,
    max_size: Option<Entry>,
    max_size_by_pattern: Vec<SizeRule>,
    secret_opt_in: Vec<Rule>,
    follow_symlinks: Vec<Rule>,
    follow_interval: Option<Entry>,
    active: Option<Entry>,
    debounce: Option<Entry>,
    rate_limit: Option<Entry>,
    periodic: Option<Entry>,
    reconcile_at_start: Option<Entry>,
    verify: Option<Entry>,
    restore_modes: Option<Entry>,
    retention: Option<Entry>,
    listener: Option<Entry>,
    port: Option<Entry>,
    allowed_origins: Option<Entry>,
    log_level: Option<Entry>,
    log_format: Option<Entry>,
    log_ceiling: Option<Entry>,
}

impl Layer {
    /// Drop the key `name` from this layer, saying whether it had set it.
    fn unset(&mut self, name: &str) -> bool {
        fn emptied<T>(list: &mut Vec<T>) -> bool {
            !std::mem::take(list).is_empty()
        }

        match name {
            "files.exclude" => emptied(&mut self.exclude),
            "files.include" => emptied(&mut self.include),
            "files.respect_gitignore" => self.respect_gitignore.take().is_some(),
            "files.max_size" => self.max_size.take().is_some(),
            "files.max_size_by_pattern" => emptied(&mut self.max_size_by_pattern),
            "files.secret_opt_in" => emptied(&mut self.secret_opt_in),
            "files.follow_symlinks" => emptied(&mut self.follow_symlinks),
            "files.follow_reconcile_interval" => self.follow_interval.take().is_some(),
            "triggers.active" => self.active.take().is_some(),
            "triggers.debounce" => self.debounce.take().is_some(),
            "triggers.rate_limit" => self.rate_limit.take().is_some(),
            "triggers.periodic" => self.periodic.take().is_some(),
            "reconcile.at_start" => self.reconcile_at_start.take().is_some(),
            "reconcile.verify" => self.verify.take().is_some(),
            "restore.modes" => self.restore_modes.take().is_some(),
            "retention.tier" => self.retention.take().is_some(),
            "ui.listener" => self.listener.take().is_some(),
            "ui.port" => self.port.take().is_some(),
            "ui.allowed_origins" => self.allowed_origins.take().is_some(),
            "log.level" => self.log_level.take().is_some(),
            "log.format" => self.log_format.take().is_some(),
            "log.ceiling" => self.log_ceiling.take().is_some(),
            other => unreachable!("`{other}` is not a key of the schema"),
        }
    }
}

/// The global configuration: the user's own file, read by the daemon alone.
///
/// Besides its own `[files]` table it carries per-Root blocks, `[root.<RootId>]`, and pattern blocks,
/// `[[roots]]` with a `path` glob, both accepting the whole schema — which is what makes a Root's own
/// file optional (ticket 04).
#[derive(Debug, Clone, Default)]
pub struct GlobalConfig {
    base: Layer,
    by_id: BTreeMap<RootId, Block>,
    patterns: Vec<(PathGlob, Block)>,
    /// The `git` maintenance runs with, when the user names one (ticket 52).
    git_path: Option<PathBuf>,
}

impl GlobalConfig {
    /// Parse the global configuration `text`, read from `path`. A pattern block's leading `~/` is the
    /// user's `home`.
    pub fn parse(path: &Path, text: &str, home: Option<&Path>) -> Result<Self, ConfigError> {
        let invalid = |message: String| ConfigError {
            path: path.to_path_buf(),
            message,
        };
        let document: GlobalDocument =
            toml::from_str(text).map_err(|error| invalid(error.to_string()))?;
        let git_path = match &document.git.path {
            Some(written) if !Path::new(written).is_absolute() => {
                return Err(invalid(format!(
                    "git.path `{written}` must be an absolute path: it is never looked up in a PATH"
                )));
            }
            written => written.as_ref().map(PathBuf::from),
        };

        let base = settings_layer(document.tables(), Level::Global, path, text, "")?;

        let mut by_id = BTreeMap::new();
        for (key, block) in &document.root {
            let id = key
                .parse::<RootId>()
                .map_err(|_| invalid(format!("[root.{key}]: `{key}` is not a Root identity")))?;
            let block_name = format!("root.{key}.");
            by_id.insert(
                id,
                Block::new(
                    settings_layer(block.tables(), Level::GlobalRoot, path, text, &block_name)?,
                    path,
                    &format!("[root.{key}]"),
                ),
            );
        }

        let mut patterns = Vec::new();
        for block in &document.roots {
            let written = block.path.get_ref();
            let expanded = expand_home(written, home).ok_or_else(|| {
                invalid(format!(
                    "[[roots]] path `{written}`: `~` names a home directory this daemon does not know"
                ))
            })?;
            if !is_absolute_glob(&expanded) {
                return Err(invalid(format!(
                    "[[roots]] path `{written}` must be absolute, or start with `~/`"
                )));
            }
            let glob = PathGlob::parse(&expanded)
                .map_err(|error| invalid(format!("[[roots]] path: {error}")))?;
            patterns.push((
                glob,
                Block::new(
                    settings_layer(block.tables(), Level::GlobalPattern, path, text, "roots.")?,
                    path,
                    &format!("the [[roots]] block for `{written}`"),
                ),
            ));
        }

        Ok(Self {
            base,
            by_id,
            patterns,
            git_path,
        })
    }
}

/// One per-Root block of the global file: what it sets, and the keys it set that describe the daemon,
/// which were ignored.
#[derive(Debug, Clone, Default)]
struct Block {
    layer: Layer,
    warnings: Vec<ConfigWarning>,
}

impl Block {
    /// The block whose settings are `layer`, in the file at `path`, as `named` names it to a person —
    /// with every key describing the daemon dropped and said to be: one daemon runs one listener,
    /// whichever Roots a block names (ticket 60).
    fn new(mut layer: Layer, path: &Path, named: &str) -> Self {
        let warnings = SCHEMA
            .iter()
            .filter(|spec| spec.daemon && layer.unset(spec.name))
            .map(|spec| ConfigWarning {
                code: WarningCode::KeyIgnored,
                message: format!(
                    "{}: {} is ignored in {named}; it describes the daemon, so only the file's own \
                     [{}] table sets it",
                    path.display(),
                    spec.name,
                    spec.name.split('.').next().unwrap_or_default(),
                ),
            })
            .collect();

        Self { layer, warnings }
    }
}

/// A Root's own `.keepster.toml`: the way to share rules through a repository.
#[derive(Debug, Clone, Default)]
pub struct RootConfig {
    layer: Layer,
    /// The keys it set that a Root's own file may not, which were ignored.
    warnings: Vec<ConfigWarning>,
}

impl RootConfig {
    /// Parse a Root's own configuration `text`, read from `path`.
    ///
    /// A key this file may not set is not an error: it is ignored, and said to be, so that a file
    /// arriving in a clone neither changes what it may not nor makes the Root `Degraded` (ticket 04).
    pub fn parse(path: &Path, text: &str) -> Result<Self, ConfigError> {
        let document: Settings = toml::from_str(text).map_err(|error| ConfigError {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

        let mut layer = settings_layer(document.tables(), Level::Root, path, text, "")?;
        let mut warnings = Vec::new();
        for spec in SCHEMA.iter().filter(|spec| !spec.root_settable) {
            if layer.unset(spec.name) {
                warnings.push(ConfigWarning {
                    code: WarningCode::KeyIgnored,
                    message: format!(
                        "{}: {} is ignored in a Root's own file; set it in the global \
                         configuration instead",
                        path.display(),
                        spec.name,
                    ),
                });
            }
        }

        Ok(Self { layer, warnings })
    }
}

/// The effective configuration of the Root `id` at `root_path`: the defaults, then the global file,
/// the last pattern block matching the Root, its block for this Root, and the Root's own file.
///
/// An id block outranks every pattern block, because the id is the Root's identity and a glob only
/// describes where it happens to be. A Root matching several pattern blocks takes the last one alone
/// (ticket 04), so a narrower block written after a broader one replaces it rather than adding to it —
/// which is what lets `~/code/android/*` say something other than `~/code/**`.
pub fn effective(
    global: &GlobalConfig,
    id: RootId,
    root_path: &Path,
    own: Option<&RootConfig>,
) -> Effective {
    let path = glob_path(root_path);
    let pattern = global
        .patterns
        .iter()
        .rev()
        .find(|(glob, _)| glob.matches(&path))
        .map(|(_, block)| block);
    let blocks: Vec<&Block> = pattern.into_iter().chain(global.by_id.get(&id)).collect();
    let layers: Vec<&Layer> = std::iter::once(&global.base)
        .chain(blocks.iter().map(|block| &block.layer))
        .chain(own.map(|own| &own.layer))
        .collect();
    let warnings = blocks
        .iter()
        .flat_map(|block| block.warnings.iter().cloned())
        .chain(own.into_iter().flat_map(|own| own.warnings.iter().cloned()))
        .collect();

    merge(&layers, warnings)
}

/// The global file at `path` as it reads now — the defaults where there is none — for a report made
/// without the daemon (ticket 63). Nothing is written and no known-good copy is fallen back on: a file
/// that does not parse is the answer.
pub fn read_global(path: Option<&Path>, home: Option<&Path>) -> Result<GlobalConfig, ConfigError> {
    let Some(path) = path else {
        return Ok(GlobalConfig::default());
    };

    match read_text(path)? {
        Some(text) => GlobalConfig::parse(path, &text, home),
        None => Ok(GlobalConfig::default()),
    }
}

/// The configuration of the Root `id` at `root_path` as its files read now beside `global`, with what
/// Forget excluded from it under `state_dir` — for a report made without the daemon (ticket 63), so,
/// as [`read_global`], it writes nothing and a file that cannot be read is the answer.
pub fn read_root(
    global: &GlobalConfig,
    id: RootId,
    root_path: &Path,
    state_dir: &Path,
) -> Resolved {
    let file = root_path.join(ROOT_CONFIG);
    let read = read_text(&file)
        .and_then(|text| text.map(|text| RootConfig::parse(&file, &text)).transpose())
        .and_then(|own| Ok((own, forgotten_at(state_dir, id)?)));
    let (own, forgotten) = match read {
        Ok(read) => read,
        Err(error) => {
            return Resolved {
                effective: None,
                errors: vec![error],
            };
        }
    };
    let mut effective = effective(global, id, root_path, own.as_ref());
    forgetting(&mut effective, forgotten);

    Resolved {
        effective: Some(effective),
        errors: Vec::new(),
    }
}

/// What Forget excluded from the Root `id` from now on, from its file under `state_dir`.
fn forgotten_at(state_dir: &Path, id: RootId) -> Result<Vec<Rule>, ConfigError> {
    let path = state_dir.join(id.to_string()).join(FORGOTTEN);
    let Some(text) = read_text(&path)? else {
        return Ok(Vec::new());
    };
    let file: ForgottenFile = toml::from_str(&text).map_err(|error| ConfigError {
        path: path.clone(),
        message: error.message().to_owned(),
    })?;

    file.patterns
        .iter()
        .map(|pattern| {
            Rule::parse(
                pattern,
                RootPath::root(),
                RuleOrigin {
                    level: Level::Forget,
                    source: path.display().to_string(),
                    line: None,
                },
            )
            .map_err(|error| ConfigError {
                path: path.clone(),
                message: error.to_string(),
            })
        })
        .collect()
}

/// The global level on its own: the defaults under the global file's own table, with no pattern block
/// and no Root's block, since each of those describes some Roots and never the global level.
pub fn global_effective(global: &GlobalConfig) -> Effective {
    merge(&[&global.base], Vec::new())
}

/// `layers`, from the least specific to the most, merged by key class, with `warnings` from reading
/// them ahead of the ones the merged rules raise.
fn merge(layers: &[&Layer], mut warnings: Vec<ConfigWarning>) -> Effective {
    let scalar = |pick: fn(&Layer) -> Option<&Entry>, default: Value| {
        layers
            .iter()
            .rev()
            .find_map(|layer| pick(layer))
            .cloned()
            .unwrap_or(Entry {
                value: default,
                origin: default_origin(),
            })
    };
    let max_size = scalar(
        |layer| layer.max_size.as_ref(),
        Value::Bytes(DEFAULT_MAX_SIZE),
    );
    let respect_gitignore = scalar(|layer| layer.respect_gitignore.as_ref(), Value::Bool(false));

    // Within a level `include` comes after `exclude`, so a level's own exceptions win over its own
    // exclude rules; across levels the more specific level wins either way.
    let rules: Vec<Rule> = layers
        .iter()
        .flat_map(|layer| layer.exclude.iter().chain(&layer.include))
        .cloned()
        .collect();
    let opt_in: Vec<Rule> = layers
        .iter()
        .flat_map(|layer| &layer.secret_opt_in)
        .cloned()
        .collect();
    let size_rules: Vec<SizeRule> = layers
        .iter()
        .flat_map(|layer| &layer.max_size_by_pattern)
        .cloned()
        .collect();
    let follow: Vec<Rule> = layers
        .iter()
        .flat_map(|layer| &layer.follow_symlinks)
        .cloned()
        .collect();

    let Value::Bytes(max) = max_size.value else {
        unreachable!("max_size is only ever set in bytes")
    };
    let Value::Bool(honour) = respect_gitignore.value else {
        unreachable!("respect_gitignore is only ever set as a switch")
    };
    let mut policy = CapturePolicy::new(
        RuleSet::new(rules),
        RuleSet::new(opt_in),
        max,
        size_rules.clone(),
    )
    .honouring_ignore_files(honour)
    .following(RuleSet::new(follow));
    if max_size.origin.level != Level::Default {
        policy = policy.with_max_size_origin(max_size.origin.clone());
    }

    let patterns = |pick: fn(&Layer) -> &Vec<Rule>| {
        Setting::List(
            layers
                .iter()
                .flat_map(|layer| pick(layer))
                .map(|rule| Entry {
                    value: Value::Pattern(rule.text.clone()),
                    origin: rule.origin.clone(),
                })
                .collect(),
        )
    };
    let size_patterns = Setting::List(
        size_rules
            .iter()
            .map(|size| Entry {
                value: Value::SizePattern {
                    pattern: size.rule.text.clone(),
                    max: (!size.rule.negated).then_some(size.max),
                },
                origin: size.rule.origin.clone(),
            })
            .collect(),
    );
    let active = scalar(
        |layer| layer.active.as_ref(),
        Value::Words(vec![Trigger::Watcher.as_str().to_owned()]),
    );
    let defaults = Triggers::default();
    let debounce = scalar(
        |layer| layer.debounce.as_ref(),
        Value::Duration(defaults.debounce),
    );
    let rate_limit = scalar(
        |layer| layer.rate_limit.as_ref(),
        Value::Duration(defaults.rate_limit),
    );
    let periodic = scalar(
        |layer| layer.periodic.as_ref(),
        Value::Duration(DEFAULT_PERIODIC),
    );
    let follow_interval = scalar(
        |layer| layer.follow_interval.as_ref(),
        Value::Duration(DEFAULT_FOLLOW_INTERVAL),
    );
    let duration = |entry: &Entry| match entry.value {
        Value::Duration(duration) => duration,
        _ => unreachable!("an interval is only ever set as a duration"),
    };
    let Value::Words(names) = &active.value else {
        unreachable!("the active Triggers are only ever set as names")
    };
    let mut triggers = Triggers {
        watcher: names.iter().any(|name| name == Trigger::Watcher.as_str()),
        periodic: names
            .iter()
            .any(|name| name == Trigger::Periodic.as_str())
            .then(|| duration(&periodic)),
        debounce: duration(&debounce),
        rate_limit: duration(&rate_limit),
        follow_every: None,
    };
    // Only for a Root captured automatically — one captured on request only is passed over on request
    // only — and never more often than the rate limit allows anyway.
    if policy.follows_any() && (triggers.watcher || triggers.periodic.is_some()) {
        triggers.follow_every = Some(duration(&follow_interval).max(triggers.rate_limit));
    }

    let at_start = scalar(|layer| layer.reconcile_at_start.as_ref(), Value::Bool(true));
    let verify = scalar(
        |layer| layer.verify.as_ref(),
        Value::Name(Verify::default().as_str().to_owned()),
    );
    let reconcile = Reconciliation {
        at_start: at_start.value == Value::Bool(true),
        verify: match &verify.value {
            Value::Name(name) => Verify::parse(name).unwrap_or_default(),
            _ => unreachable!("the verification mode is only ever set as a name"),
        },
        fallback_every: duration(&periodic),
    };
    let restore_modes = scalar(
        |layer| layer.restore_modes.as_ref(),
        Value::Name(ASK.to_owned()),
    );
    let retention = scalar(
        |layer| layer.retention.as_ref(),
        Value::Ladder(Ladder::default()),
    );
    let Value::Ladder(ladder) = retention.value.clone() else {
        unreachable!("the ladder is only ever set as a ladder")
    };
    let listener = scalar(|layer| layer.listener.as_ref(), Value::Bool(true));
    let port = scalar(
        |layer| layer.port.as_ref(),
        Value::Number(u64::from(DEFAULT_PORT)),
    );
    let allowed_origins = scalar(
        |layer| layer.allowed_origins.as_ref(),
        Value::Words(Vec::new()),
    );
    let log_level = scalar(
        |layer| layer.log_level.as_ref(),
        Value::Name("info".to_owned()),
    );
    let log_format = scalar(
        |layer| layer.log_format.as_ref(),
        Value::Name("text".to_owned()),
    );
    let log_ceiling = scalar(
        |layer| layer.log_ceiling.as_ref(),
        Value::Bytes(DEFAULT_LOG_CEILING),
    );
    let modes_default = match &restore_modes.value {
        Value::Name(name) => mode_choice(name).flatten(),
        _ => unreachable!("the Restore modes answer is only ever set as a name"),
    };

    let settings = [
        ("files.exclude", patterns(|layer| &layer.exclude)),
        ("files.include", patterns(|layer| &layer.include)),
        (
            "files.respect_gitignore",
            Setting::Scalar(respect_gitignore),
        ),
        ("files.max_size", Setting::Scalar(max_size)),
        ("files.max_size_by_pattern", size_patterns),
        (
            "files.secret_opt_in",
            patterns(|layer| &layer.secret_opt_in),
        ),
        (
            "files.follow_symlinks",
            patterns(|layer| &layer.follow_symlinks),
        ),
        (
            "files.follow_reconcile_interval",
            Setting::Scalar(follow_interval),
        ),
        ("triggers.active", Setting::Scalar(active)),
        ("triggers.debounce", Setting::Scalar(debounce)),
        ("triggers.rate_limit", Setting::Scalar(rate_limit)),
        ("triggers.periodic", Setting::Scalar(periodic)),
        ("reconcile.at_start", Setting::Scalar(at_start)),
        ("reconcile.verify", Setting::Scalar(verify)),
        ("restore.modes", Setting::Scalar(restore_modes)),
        ("retention.tier", Setting::Scalar(retention)),
        ("ui.listener", Setting::Scalar(listener)),
        ("ui.port", Setting::Scalar(port)),
        ("ui.allowed_origins", Setting::Scalar(allowed_origins)),
        ("log.level", Setting::Scalar(log_level)),
        ("log.format", Setting::Scalar(log_format)),
        ("log.ceiling", Setting::Scalar(log_ceiling)),
    ];
    let keys = settings
        .into_iter()
        .map(|(name, setting)| EffectiveKey {
            spec: spec(name),
            setting,
            restart_required: false,
        })
        .collect();

    warnings.extend(unreachable_warnings(&policy));

    Effective {
        policy,
        triggers,
        reconcile,
        restore_modes: modes_default,
        retention: ladder,
        keys,
        warnings,
    }
}

/// The origin `written` names, as a browser sends it in `Origin` — lowercase, `scheme://host[:port]` —
/// or why it is not one.
///
/// Never a wildcard, in whole or in part: an allowed origin is a promise about one page, and `*` would
/// hand the loopback listener to every page the user has open (ticket 07).
fn allowed_origin(written: &str) -> Result<String, String> {
    let refused = |why: &str| Err(format!("`{written}` {why}"));
    if written.contains('*') {
        return refused("is a wildcard, and an allowed origin never is: name each origin");
    }
    let origin = written.to_ascii_lowercase();
    let Some((scheme, authority)) = origin.split_once("://") else {
        return refused("is not an origin; write one as `http://localhost:3000`");
    };
    if scheme != "http" && scheme != "https" {
        return refused("is not an origin: its scheme is neither `http` nor `https`");
    }
    if authority.contains(['/', '?', '#', '@']) {
        return refused("is not an origin: it holds more than a scheme, a host and a port");
    }
    // An IPv6 host is bracketed, and its own colons are not a port's.
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, "")) => (host, None),
            Some((host, rest)) => (host, Some(rest.strip_prefix(':').unwrap_or(rest))),
            None => return refused("is not an origin: its IPv6 host is never closed"),
        },
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    if port.is_some_and(|port| port.parse::<u16>().is_err()) {
        return refused("is not an origin: its port is not a number from 0 to 65535");
    }
    if host.is_empty() {
        return refused("is not an origin: it names no host");
    }

    Ok(origin)
}

/// `restore.modes`' value for asking, its default.
const ASK: &str = "ask";

/// What `restore.modes` set to `name` answers: `Some(None)` for asking, and `None` for a name it does
/// not know.
fn mode_choice(name: &str) -> Option<Option<ModeChoice>> {
    match name {
        ASK => Some(None),
        "snapshot" => Some(Some(ModeChoice::Snapshot)),
        "keep" => Some(Some(ModeChoice::Keep)),
        _ => None,
    }
}

fn default_origin() -> RuleOrigin {
    RuleOrigin {
        level: Level::Default,
        source: "the built-in default".to_owned(),
        line: None,
    }
}

/// A warning for every negation an excluded ancestor makes unreachable (ADR 0007).
fn unreachable_warnings(policy: &CapturePolicy) -> Vec<ConfigWarning> {
    policy
        .rules()
        .unreachable_negations()
        .into_iter()
        .map(|unreachable| ConfigWarning {
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
        })
        .collect()
}

/// `written` with a leading `~/` replaced by `home`, or `None` when it needs a home and there is none.
fn expand_home(written: &str, home: Option<&Path>) -> Option<String> {
    match written.strip_prefix("~/") {
        Some(rest) => Some(format!("{}/{rest}", glob_path(home?).trim_end_matches('/'))),
        None if written == "~" => home.map(glob_path),
        None => Some(written.to_owned()),
    }
}

/// Whether a pattern block's glob names an absolute path: `/…`, or a drive such as `C:/…`.
fn is_absolute_glob(glob: &str) -> bool {
    let bytes = glob.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/';

    glob.starts_with('/') || drive
}

/// A directory path as a pattern block's glob is matched against: `/`-separated on every platform.
fn glob_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GlobalDocument {
    // The whole schema at the top level, then the two kinds of per-Root block, each of which accepts the
    // whole schema too. A table added to the schema is added to `Settings` and to both blocks.
    #[serde(default)]
    files: Files,
    #[serde(default)]
    triggers: TriggerTable,
    #[serde(default)]
    reconcile: ReconcileTable,
    #[serde(default)]
    restore: RestoreTable,
    #[serde(default)]
    retention: RetentionTable,
    #[serde(default)]
    ui: UiTable,
    #[serde(default)]
    log: LogTable,
    // The daemon's own, so the top level alone holds it: one daemon runs one `git` for every Root.
    #[serde(default)]
    git: GitTable,
    #[serde(default)]
    root: BTreeMap<String, Settings>,
    #[serde(default)]
    roots: Vec<PatternBlock>,
}

/// The whole schema: what a Root's own file holds, and what a `[root.<RootId>]` block holds.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    #[serde(default)]
    files: Files,
    #[serde(default)]
    triggers: TriggerTable,
    #[serde(default)]
    reconcile: ReconcileTable,
    #[serde(default)]
    restore: RestoreTable,
    #[serde(default)]
    retention: RetentionTable,
    #[serde(default)]
    ui: UiTable,
    #[serde(default)]
    log: LogTable,
}

/// A `[[roots]]` block: the Roots its `path` glob matches, and the whole schema for them.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PatternBlock {
    path: Spanned<String>,
    #[serde(default)]
    files: Files,
    #[serde(default)]
    triggers: TriggerTable,
    #[serde(default)]
    reconcile: ReconcileTable,
    #[serde(default)]
    restore: RestoreTable,
    #[serde(default)]
    retention: RetentionTable,
    #[serde(default)]
    ui: UiTable,
    #[serde(default)]
    log: LogTable,
}

/// The tables of the schema, wherever a block holds them.
struct Tables<'a> {
    files: &'a Files,
    triggers: &'a TriggerTable,
    reconcile: &'a ReconcileTable,
    restore: &'a RestoreTable,
    retention: &'a RetentionTable,
    ui: &'a UiTable,
    log: &'a LogTable,
}

/// Each block holding the whole schema hands its tables over as one.
macro_rules! holds_tables {
    ($($block:ty),*) => {$(
        impl $block {
            fn tables(&self) -> Tables<'_> {
                Tables {
                    files: &self.files,
                    triggers: &self.triggers,
                    reconcile: &self.reconcile,
                    restore: &self.restore,
                    retention: &self.retention,
                    ui: &self.ui,
                    log: &self.log,
                }
            }
        }
    )*};
}

holds_tables!(GlobalDocument, Settings, PatternBlock);

/// The `[git]` table of the global file: the `git` maintenance runs with, authoritatively when set — one
/// that does not work is reported, never replaced by another (ticket 11).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct GitTable {
    path: Option<String>,
}

/// The `[ui]` table: the loopback listener the browser reaches the daemon through (ticket 60). Every
/// key of it describes the daemon, so only the global file's own table sets them; anywhere else they
/// are read, so that a block or a Root's file setting one is told so, and then ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UiTable {
    listener: Option<Spanned<bool>>,
    port: Option<Spanned<i64>>,
    allowed_origins: Option<Spanned<Vec<String>>>,
}

/// The `[log]` table: the daemon's log (ticket 62). Daemon keys, read everywhere so that a block or a
/// Root's file setting one is told so, and then ignored.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogTable {
    level: Option<Spanned<String>>,
    format: Option<Spanned<String>>,
    ceiling: Option<Spanned<String>>,
}

/// The `[retention]` table: the ladder, one `[[retention.tier]]` per Tier from the youngest (ticket 51).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionTable {
    #[serde(default)]
    tier: Vec<Spanned<TierEntry>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TierEntry {
    age: String,
    keep: String,
}

/// The `[restore]` table: what a Restore does where the request does not say (ticket 50).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreTable {
    modes: Option<Spanned<String>>,
}

/// The `[reconcile]` table: whether a pass runs at the start, and how it judges a file (ticket 44).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReconcileTable {
    at_start: Option<Spanned<bool>>,
    verify: Option<Spanned<String>>,
}

/// The `[triggers]` table: which Triggers are active, and their intervals (ticket 43).
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerTable {
    active: Option<Spanned<Vec<String>>>,
    debounce: Option<Spanned<String>>,
    rate_limit: Option<Spanned<String>>,
    periodic: Option<Spanned<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Files {
    #[serde(default)]
    exclude: Vec<Spanned<String>>,
    #[serde(default)]
    include: Vec<Spanned<String>>,
    respect_gitignore: Option<Spanned<bool>>,
    max_size: Option<Spanned<String>>,
    #[serde(default)]
    max_size_by_pattern: Vec<Spanned<SizeEntry>>,
    #[serde(default)]
    secret_opt_in: Vec<Spanned<String>>,
    #[serde(default)]
    follow_symlinks: Vec<Spanned<String>>,
    follow_reconcile_interval: Option<Spanned<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SizeEntry {
    pattern: String,
    /// Required, except for a negated pattern, which returns its paths to the scalar cap.
    max: Option<String>,
}

/// Check the tables of one block of the file at `path` — the top level, or a per-Root block named by
/// `block` — and compile what they say, as the settings of `level`.
fn settings_layer(
    Tables {
        files,
        triggers,
        reconcile,
        restore,
        retention,
        ui,
        log,
    }: Tables<'_>,
    level: Level,
    path: &Path,
    text: &str,
    block: &str,
) -> Result<Layer, ConfigError> {
    let mut layer = layer(files, level, path, text, &format!("{block}files"))?;

    let origin = |span: std::ops::Range<usize>| RuleOrigin {
        level,
        source: path.to_string_lossy().into_owned(),
        line: Some(line_of(text, span.start)),
    };
    let invalid = |key: &str, message: String| ConfigError {
        path: path.to_path_buf(),
        message: format!("{block}triggers.{key}: {message}"),
    };
    let interval = |key: &str, written: &Option<Spanned<String>>, zero_allowed: bool| {
        written
            .as_ref()
            .map(|written| {
                let duration = parse_duration(written.get_ref())
                    .map_err(|error| invalid(key, error.to_string()))?;
                if duration.is_zero() && !zero_allowed {
                    return Err(invalid(key, "it cannot be zero".to_owned()));
                }
                Ok(Entry {
                    value: Value::Duration(duration),
                    origin: origin(written.span()),
                })
            })
            .transpose()
    };

    layer.debounce = interval("debounce", &triggers.debounce, true)?;
    // Always in force, so a rate limit of nothing is refused rather than meaning "none".
    layer.rate_limit = interval("rate_limit", &triggers.rate_limit, false)?;
    layer.periodic = interval("periodic", &triggers.periodic, false)?;
    layer.reconcile_at_start = reconcile.at_start.as_ref().map(|at_start| Entry {
        value: Value::Bool(*at_start.get_ref()),
        origin: origin(at_start.span()),
    });
    layer.verify = reconcile
        .verify
        .as_ref()
        .map(|verify| match Verify::parse(verify.get_ref()) {
            Some(mode) => Ok(Entry {
                value: Value::Name(mode.as_str().to_owned()),
                origin: origin(verify.span()),
            }),
            None => Err(ConfigError {
                path: path.to_path_buf(),
                message: format!(
                    "{block}reconcile.verify: `{}` is not a mode; write `stat` or `hash`",
                    verify.get_ref()
                ),
            }),
        })
        .transpose()?;
    layer.restore_modes = restore
        .modes
        .as_ref()
        .map(|modes| match mode_choice(modes.get_ref()) {
            Some(_) => Ok(Entry {
                value: Value::Name(modes.get_ref().clone()),
                origin: origin(modes.span()),
            }),
            None => Err(ConfigError {
                path: path.to_path_buf(),
                message: format!(
                    "{block}restore.modes: `{}` is not an answer; write `ask`, `snapshot` or `keep`",
                    modes.get_ref()
                ),
            }),
        })
        .transpose()?;
    layer.retention = retention
        .tier
        .first()
        .map(|first| {
            let invalid = |message: String| ConfigError {
                path: path.to_path_buf(),
                message: format!("{block}retention.tier: {message}"),
            };
            let tiers = retention
                .tier
                .iter()
                .map(|tier| {
                    let TierEntry { age, keep } = tier.get_ref();
                    Ok(keepster_core::Tier {
                        age: parse_duration(age).map_err(|error| invalid(error.to_string()))?,
                        keep: Keep::parse(keep).ok_or_else(|| {
                            invalid(format!(
                                "`{keep}` is not what a Tier keeps; write `all`, `hourly`, `daily`, \
                                 `weekly` or a period such as `6h`"
                            ))
                        })?,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let ladder = Ladder::new(tiers).map_err(|error| invalid(error.to_string()))?;
            Ok(Entry {
                value: Value::Ladder(ladder),
                origin: origin(first.span()),
            })
        })
        .transpose()?;
    let invalid_ui = |key: &str, message: String| ConfigError {
        path: path.to_path_buf(),
        message: format!("{block}ui.{key}: {message}"),
    };
    layer.listener = ui.listener.as_ref().map(|listener| Entry {
        value: Value::Bool(*listener.get_ref()),
        origin: origin(listener.span()),
    });
    layer.port = ui
        .port
        .as_ref()
        .map(|port| match u16::try_from(*port.get_ref()) {
            Ok(number) if number != 0 => Ok(Entry {
                value: Value::Number(u64::from(number)),
                origin: origin(port.span()),
            }),
            _ => Err(invalid_ui(
                "port",
                format!(
                    "`{}` is not a port; write one from 1 to 65535",
                    port.get_ref()
                ),
            )),
        })
        .transpose()?;
    layer.allowed_origins = ui
        .allowed_origins
        .as_ref()
        .map(|origins| {
            let mut allowed: Vec<String> = Vec::new();
            for written in origins.get_ref() {
                let origin = allowed_origin(written)
                    .map_err(|message| invalid_ui("allowed_origins", message))?;
                if !allowed.contains(&origin) {
                    allowed.push(origin);
                }
            }
            Ok(Entry {
                value: Value::Words(allowed),
                origin: origin(origins.span()),
            })
        })
        .transpose()?;
    let invalid_log = |key: &str, message: String| ConfigError {
        path: path.to_path_buf(),
        message: format!("{block}log.{key}: {message}"),
    };
    let one_of = |key: &str, written: &Option<Spanned<String>>, known: &[&str]| {
        written
            .as_ref()
            .map(|written| {
                let name = written.get_ref();
                if known.contains(&name.as_str()) {
                    Ok(Entry {
                        value: Value::Name(name.clone()),
                        origin: origin(written.span()),
                    })
                } else {
                    Err(invalid_log(
                        key,
                        format!("`{name}` is not one; write {}", known.join(", ")),
                    ))
                }
            })
            .transpose()
    };
    layer.log_level = one_of("level", &log.level, LOG_LEVELS)?;
    layer.log_format = one_of("format", &log.format, LOG_FORMATS)?;
    layer.log_ceiling = log
        .ceiling
        .as_ref()
        .map(|ceiling| {
            let bytes = parse_size(ceiling.get_ref())
                .map_err(|error| invalid_log("ceiling", error.to_string()))?;
            if bytes < MINIMUM_LOG_CEILING {
                return Err(invalid_log(
                    "ceiling",
                    format!(
                        "`{}` is below the smallest ceiling, {MINIMUM_LOG_CEILING} bytes",
                        ceiling.get_ref()
                    ),
                ));
            }
            Ok(Entry {
                value: Value::Bytes(bytes),
                origin: origin(ceiling.span()),
            })
        })
        .transpose()?;
    layer.active = triggers
        .active
        .as_ref()
        .map(|active| {
            let known = [Trigger::Watcher, Trigger::Periodic].map(Trigger::as_str);
            let mut names: Vec<String> = Vec::new();
            for name in active.get_ref() {
                if !known.contains(&name.as_str()) {
                    return Err(invalid(
                        "active",
                        format!(
                            "`{name}` is not a Trigger; the Triggers are `watcher` and `periodic`"
                        ),
                    ));
                }
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
            Ok(Entry {
                value: Value::Words(names),
                origin: origin(active.span()),
            })
        })
        .transpose()?;

    Ok(layer)
}

/// Check one `[files]` table of the file at `path` and compile what it says, as the settings of
/// `level`. `table` is how a message names the table: `files`, or the block it sits in.
fn layer(
    files: &Files,
    level: Level,
    path: &Path,
    text: &str,
    table: &str,
) -> Result<Layer, ConfigError> {
    type Compile = fn(&str, RootPath, RuleOrigin) -> Result<Rule, InvalidPattern>;

    let origin = |span: std::ops::Range<usize>| RuleOrigin {
        level,
        source: path.to_string_lossy().into_owned(),
        line: Some(line_of(text, span.start)),
    };
    let invalid = |key: &str, message: String| ConfigError {
        path: path.to_path_buf(),
        message: format!("{table}.{key}: {message}"),
    };
    let rules = |key: &str, patterns: &[Spanned<String>], compile: Compile| {
        patterns
            .iter()
            .map(|pattern| {
                compile(pattern.get_ref(), RootPath::root(), origin(pattern.span()))
                    .map_err(|error| invalid(key, error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()
    };

    let mut max_size_by_pattern = Vec::new();
    for entry in &files.max_size_by_pattern {
        let rule = Rule::parse(
            &entry.get_ref().pattern,
            RootPath::root(),
            origin(entry.span()),
        )
        .map_err(|error| invalid("max_size_by_pattern", error.to_string()))?;
        let max = match (&entry.get_ref().max, rule.negated) {
            (Some(max), _) => parse_size(max)
                .map_err(|error| invalid("max_size_by_pattern", error.to_string()))?,
            (None, true) => 0,
            (None, false) => {
                return Err(invalid(
                    "max_size_by_pattern",
                    format!("`{}` needs a `max`", entry.get_ref().pattern),
                ));
            }
        };
        max_size_by_pattern.push(SizeRule { rule, max });
    }

    let max_size = files
        .max_size
        .as_ref()
        .map(|size| {
            parse_size(size.get_ref())
                .map(|bytes| Entry {
                    value: Value::Bytes(bytes),
                    origin: origin(size.span()),
                })
                .map_err(|error| invalid("max_size", error.to_string()))
        })
        .transpose()?;

    let follow_interval = files
        .follow_reconcile_interval
        .as_ref()
        .map(|interval| {
            let duration = parse_duration(interval.get_ref())
                .map_err(|error| invalid("follow_reconcile_interval", error.to_string()))?;
            if duration.is_zero() {
                return Err(invalid(
                    "follow_reconcile_interval",
                    "it cannot be zero".to_owned(),
                ));
            }
            Ok(Entry {
                value: Value::Duration(duration),
                origin: origin(interval.span()),
            })
        })
        .transpose()?;

    Ok(Layer {
        exclude: rules("exclude", &files.exclude, Rule::parse)?,
        include: rules("include", &files.include, Rule::include)?,
        respect_gitignore: files.respect_gitignore.as_ref().map(|respect| Entry {
            value: Value::Bool(*respect.get_ref()),
            origin: origin(respect.span()),
        }),
        max_size,
        max_size_by_pattern,
        secret_opt_in: rules("secret_opt_in", &files.secret_opt_in, Rule::parse)?,
        follow_symlinks: rules("follow_symlinks", &files.follow_symlinks, Rule::parse)?,
        follow_interval,
        ..Layer::default()
    })
}

/// Where the global file's last known-good version is kept, under the daemon's state directory.
const GLOBAL_KNOWN_GOOD: &str = "config.known-good.toml";

/// Where a Root's own file's last known-good version is kept, in that Root's store directory.
const ROOT_KNOWN_GOOD: &str = "keepster.known-good.toml";

/// What Forget excluded from a Root from now on (ticket 81), beside its record: a file the daemon owns
/// and writes, never one a person keeps in git, so configuration stays read-only to the daemon
/// (ticket 07).
const FORGOTTEN: &str = "forgotten.toml";

/// The shape of [`FORGOTTEN`].
#[derive(Debug, Default, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ForgottenFile {
    /// Every pattern excluded from now on, as Forget was given it.
    patterns: Vec<String>,
}

/// The configuration the daemon holds: every file's last known-good version, and what is wrong with
/// any whose current version is not applied.
///
/// Files are re-read whenever a Root's configuration is resolved, and compared with what was read last,
/// so an edit to either level takes effect at the next request or the next tick — nothing has to be
/// restarted or told. An invalid version is never applied: the last known-good stays in force, and is
/// kept on disk too, so a typo followed by a reboot leaves the Root `Degraded` rather than unwatched
/// (ticket 04).
#[derive(Debug)]
pub struct Configuration {
    global_path: Option<PathBuf>,
    home: Option<PathBuf>,
    state_dir: PathBuf,
    global: Tracked<GlobalConfig>,
    roots: HashMap<RootId, TrackedRoot>,
    /// Roots whose classification inputs changed, for ticket 44's Reconciliation to take.
    reconcile: BTreeSet<RootId>,
    /// The values the keys that cannot change live had when the daemon started — taken from the first
    /// version of the global file in force — which stay in force until it restarts.
    at_start: Option<BTreeMap<&'static str, Setting>>,
}

/// One file as last read, and the version of it in force.
#[derive(Debug)]
struct Tracked<T> {
    /// The text last read — `Some(None)` for a file that was absent — or `None` before the first
    /// read and after one that failed, so the next resolution reads it again.
    seen: Option<Option<String>>,
    /// The last known-good version, or `None` when no version ever was.
    in_force: Option<T>,
    /// Why the current version is not the one in force.
    error: Option<ConfigError>,
}

impl<T> Default for Tracked<T> {
    fn default() -> Self {
        Self {
            seen: None,
            in_force: None,
            error: None,
        }
    }
}

#[derive(Debug, Default)]
struct TrackedRoot {
    path: PathBuf,
    /// The Root's own file; a known-good of `None` is a Root without one.
    file: Tracked<Option<RootConfig>>,
    /// What classified paths at the last resolution, to tell whether a reload changed it.
    inputs: Option<Vec<(&'static str, Vec<Value>)>>,
}

/// What the daemon runs with, from the keys describing it (ticket 60).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonSettings {
    /// Whether the loopback listener exists at all: `ui.listener`.
    pub listener: bool,
    /// The port it binds: `ui.port`.
    pub port: u16,
    /// The browser origins it answers besides its own, each as `scheme://host[:port]`:
    /// `ui.allowed_origins`.
    pub allowed_origins: Vec<String>,
    /// The level its log returns to after a raise expires: `log.level`, one of [`LOG_LEVELS`].
    pub log_level: String,
    /// How a log line is written: `log.format`, one of [`LOG_FORMATS`].
    pub log_format: String,
    /// How much the log's files hold together, in bytes: `log.ceiling`.
    pub log_ceiling: u64,
}

/// The global level of the configuration as the daemon has it in force.
#[derive(Debug, Clone)]
pub struct GlobalLevel {
    /// The global file, where there is one.
    pub file: Option<PathBuf>,
    /// The level's effective configuration, or `None` while the global file never had a valid version.
    pub effective: Option<Effective>,
    /// Why the global file's current version is not applied, when it is not.
    pub error: Option<ConfigError>,
}

/// One Root's configuration as it stands.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The configuration in force, or `None` when some file it depends on never had a valid version —
    /// in which case the Root is not captured at all, since falling back to the defaults would drop the
    /// user's exclude rules (ticket 04).
    pub effective: Option<Effective>,
    /// The files whose current version is invalid and therefore not applied. Any makes the Root
    /// `Degraded`.
    pub errors: Vec<ConfigError>,
}

impl Configuration {
    /// The configuration of a daemon whose global file is `global_path` (if any), whose user's home is
    /// `home`, and whose state — the Registry and the known-good copies — lives under `state_dir`.
    ///
    /// The global file is read here, so that what the daemon starts with is what a key that cannot
    /// change live keeps until the next start.
    pub fn new(global_path: Option<PathBuf>, home: Option<PathBuf>, state_dir: PathBuf) -> Self {
        let mut configuration = Self {
            global_path,
            home,
            state_dir,
            global: Tracked::default(),
            roots: HashMap::new(),
            reconcile: BTreeSet::new(),
            at_start: None,
        };
        configuration.refresh_global();

        configuration
    }

    /// The configuration of the Root `id` at `root_path`, re-reading whichever of its files changed.
    pub fn resolve(&mut self, id: RootId, root_path: &Path) -> Resolved {
        self.refresh_global();
        self.refresh_root(id, root_path);

        let Some(global) = self.global.in_force.as_ref() else {
            return self.unresolved(id);
        };
        let Some(Some(own)) = self.roots.get(&id).map(|root| root.file.in_force.as_ref()) else {
            return self.unresolved(id);
        };
        let mut effective = effective(global, id, root_path, own.as_ref());
        // Without what Forget excluded, a capture would take a forgotten secret straight back: a file
        // that cannot be read leaves the Root uncaptured rather than capture it on terms it does not
        // state.
        let forgotten = match self.forgotten(id) {
            Ok(forgotten) => forgotten,
            Err(error) => {
                let mut errors = self.errors(id);
                errors.push(error);
                return Resolved {
                    effective: None,
                    errors,
                };
            }
        };
        forgetting(&mut effective, forgotten);
        effective.policy = std::mem::take(&mut effective.policy)
            .with_fences(
                history_fence(&self.state_dir, root_path)
                    .into_iter()
                    .collect(),
            )
            .with_real_fences(vec![(
                resolved(&self.state_dir),
                WithheldReason::HistoryStore,
            )]);

        self.as_started(&mut effective);

        let inputs: Vec<(&'static str, Vec<Value>)> = effective
            .classification_inputs()
            .into_iter()
            .map(|(name, values)| (name, values.into_iter().cloned().collect()))
            .collect();
        let root = self.roots.get_mut(&id).expect("refreshed above");
        if root
            .inputs
            .as_ref()
            .is_some_and(|previous| *previous != inputs)
        {
            self.reconcile.insert(id);
        }
        root.inputs = Some(inputs);

        Resolved {
            effective: Some(effective),
            errors: self.errors(id),
        }
    }

    /// The global level on its own as the daemon has it in force: the defaults under the global file's
    /// own table, with the keys that cannot change live as they stood at the start — what a diagnostics
    /// bundle reports beside every Root's configuration, so the keys describing the daemon are there
    /// even with no Root at all (ticket 63).
    pub fn global(&mut self) -> GlobalLevel {
        self.refresh_global();
        let effective = self.global.in_force.as_ref().map(|global| {
            let mut effective = global_effective(global);
            self.as_started(&mut effective);
            effective
        });

        GlobalLevel {
            file: self.global_path.clone(),
            effective,
            error: self.global.error.clone(),
        }
    }

    /// The configuration a directory would be registered under, read now: the global file in force and
    /// the directory's own `.keepster.toml`, exactly as registering it would read them (ticket 57).
    ///
    /// For a Capture Preview of a directory that is not a Root. `unwatched` is the identity of the Root
    /// once watched at `path`, which registering it again would take back — and with it the global file's
    /// block for that Root. Nothing is remembered: a candidate has no known-good to fall back on, so a
    /// file that cannot be read is the answer, not the defaults.
    pub fn candidate(
        &mut self,
        path: &Path,
        unwatched: Option<RootId>,
    ) -> Result<Effective, Vec<ConfigError>> {
        self.refresh_global();
        let Some(global) = self.global.in_force.as_ref() else {
            return Err(self.global.error.iter().cloned().collect());
        };
        let file = path.join(ROOT_CONFIG);
        let own = read_text(&file)
            .and_then(|text| text.map(|text| RootConfig::parse(&file, &text)).transpose())
            .map_err(|error| vec![error])?;
        // A Root registered now has an identity no block names, so none applies to it — and nothing
        // was ever forgotten from it.
        let id = unwatched.unwrap_or_else(|| RootId::generate(UNIX_EPOCH));
        let mut effective = effective(global, id, path, own.as_ref());
        if unwatched.is_some() {
            // Taken back with the identity, as registering again takes them back.
            let forgotten = self.forgotten(id).map_err(|error| vec![error])?;
            forgetting(&mut effective, forgotten);
        }
        effective.policy = std::mem::take(&mut effective.policy)
            .with_fences(history_fence(&self.state_dir, path).into_iter().collect())
            .with_real_fences(vec![(
                resolved(&self.state_dir),
                WithheldReason::HistoryStore,
            )]);

        Ok(effective)
    }

    /// Forget every Root `keep` does not name, once they are deregistered.
    pub fn retain(&mut self, keep: impl Fn(RootId) -> bool) {
        self.roots.retain(|id, _| keep(*id));
        self.reconcile.retain(|id| keep(*id));
    }

    /// The Roots whose classification inputs changed since the last time this was asked, each owed a
    /// full comparison against its Working Tree — ticket 44's Reconciliation takes them from here.
    pub fn take_reconcile_queue(&mut self) -> BTreeSet<RootId> {
        std::mem::take(&mut self.reconcile)
    }

    /// Put the keys that cannot change live in `effective` back to the values they had at the start,
    /// marking each a file now sets otherwise as needing a restart.
    fn as_started(&self, effective: &mut Effective) {
        let Some(started) = &self.at_start else {
            return;
        };
        let now = self.daemon_settings();
        for key in effective.keys.iter_mut().filter(|key| !key.spec.live) {
            if let Some(setting) = started.get(key.spec.name) {
                key.restart_required =
                    now.get(key.spec.name).map(Setting::values) != Some(setting.values());
                key.setting = setting.clone();
            }
        }
    }

    /// The keys that cannot change live as the global file's own table sets them now. Such a key
    /// describes the daemon, one value per process, so no per-Root block decides it.
    fn daemon_settings(&self) -> BTreeMap<&'static str, Setting> {
        match &self.global.in_force {
            Some(global) => not_live(&global.base),
            None => BTreeMap::new(),
        }
    }

    /// What the daemon runs with: the keys describing it as they stood when it started, which stay in
    /// force until the next start whatever the file says meanwhile (ticket 60).
    ///
    /// A global file with no valid version at the start leaves the defaults in force — and they, not
    /// whichever version is written next, are what a later edit is compared against.
    pub fn daemon(&mut self) -> DaemonSettings {
        self.refresh_global();
        let started = self
            .at_start
            .get_or_insert_with(|| not_live(&Layer::default()));

        settings_of(started)
    }

    /// The ladder a Root no longer watched decays by: the global file's, with its blocks for the Root —
    /// never the Root's own file, which may have gone with its directory (ticket 53). `None` while the
    /// global file has no valid version: the ladder that cannot be read is the one it would be thinned
    /// by.
    pub fn unwatched_ladder(&mut self, id: RootId, root_path: &Path) -> Option<Ladder> {
        self.refresh_global();

        self.global
            .in_force
            .as_ref()
            .map(|global| effective(global, id, root_path, None).retention)
    }

    /// What Forget excluded from the Root `id` from now on, as rules of their own level, above every
    /// level a person writes (ticket 81).
    pub fn forgotten(&self, id: RootId) -> Result<Vec<Rule>, ConfigError> {
        forgotten_at(&self.state_dir, id)
    }

    /// Exclude `pattern` from the Root `id` from now on, as Forget's `--exclude` asks: added once to the
    /// file the daemon owns, and in force at the next resolution.
    pub fn forget_from_now_on(&mut self, id: RootId, pattern: &str) -> Result<(), ConfigError> {
        let directory = self.state_dir.join(id.to_string());
        let path = directory.join(FORGOTTEN);
        let failed = |error: io::Error| ConfigError {
            path: path.clone(),
            message: format!("it could not be written: {error}"),
        };
        let mut file = match read_text(&path)? {
            Some(text) => toml::from_str::<ForgottenFile>(&text).map_err(|error| ConfigError {
                path: path.clone(),
                message: error.message().to_owned(),
            })?,
            None => ForgottenFile::default(),
        };
        if file.patterns.iter().any(|known| known == pattern) {
            return Ok(());
        }
        file.patterns.push(pattern.to_owned());
        let text = format!(
            "# Written by `keepster forget --exclude`: paths kept out of history from now on, above every\n\
             # rule a configuration file sets. Forgetting is the daemon's to record; edit with care.\n{}",
            toml::to_string_pretty(&file).map_err(|error| ConfigError {
                path: path.clone(),
                message: error.to_string(),
            })?
        );
        fs::create_dir_all(&directory).map_err(failed)?;

        write_atomically(&path, &text).map_err(failed)
    }

    /// The `git` the global file in force names, if it names one.
    pub fn git_path(&mut self) -> Option<PathBuf> {
        self.refresh_global();
        self.global
            .in_force
            .as_ref()
            .and_then(|global| global.git_path.clone())
    }

    /// Re-read the global file if it changed, keeping its last known-good version in force.
    pub fn refresh_global(&mut self) {
        self.refresh_global_file();
        if self.at_start.is_none() && self.global.in_force.is_some() {
            self.at_start = Some(self.daemon_settings());
        }
    }

    fn unresolved(&self, id: RootId) -> Resolved {
        Resolved {
            effective: None,
            errors: self.errors(id),
        }
    }

    fn errors(&self, id: RootId) -> Vec<ConfigError> {
        self.global
            .error
            .iter()
            .chain(
                self.roots
                    .get(&id)
                    .and_then(|root| root.file.error.as_ref()),
            )
            .cloned()
            .collect()
    }

    fn refresh_global_file(&mut self) {
        let Some(path) = self.global_path.clone() else {
            self.global
                .in_force
                .get_or_insert_with(GlobalConfig::default);
            return;
        };
        let home = self.home.clone();
        let known_good = self.state_dir.join(GLOBAL_KNOWN_GOOD);

        refresh(&mut self.global, &path, &known_good, |text| match text {
            Some(text) => GlobalConfig::parse(&path, text, home.as_deref()),
            None => Ok(GlobalConfig::default()),
        });
    }

    fn refresh_root(&mut self, id: RootId, root_path: &Path) {
        let root = self.roots.entry(id).or_default();
        if root.path != root_path {
            // A moved Root has a different file; its known-good stays the fallback until the new one
            // is read.
            root.path = root_path.to_path_buf();
            root.file.seen = None;
        }
        let path = root_path.join(ROOT_CONFIG);
        let known_good = self.state_dir.join(id.to_string()).join(ROOT_KNOWN_GOOD);
        let parse =
            |text: Option<&str>| text.map(|text| RootConfig::parse(&path, text)).transpose();

        // A Root whose directory is gone has a file that is absent for that reason alone, which says
        // nothing about its configuration: reading it as absent would replace the known-good with the
        // defaults. What was in force — or kept on disk — stays in force until the directory is back.
        if !root_path.is_dir() {
            if root.file.in_force.is_none() {
                root.file.in_force = recovered(&known_good, &parse);
            }
            return;
        }

        refresh(&mut root.file, &path, &known_good, parse);
    }
}

/// The daemon's settings from the keys describing it, each absent one at its default.
fn settings_of(started: &BTreeMap<&'static str, Setting>) -> DaemonSettings {
    let value = |name: &str| {
        started
            .get(name)
            .and_then(|setting| setting.values().first().copied().cloned())
    };

    DaemonSettings {
        listener: value("ui.listener") != Some(Value::Bool(false)),
        port: match value("ui.port") {
            Some(Value::Number(port)) => u16::try_from(port).unwrap_or(DEFAULT_PORT),
            _ => DEFAULT_PORT,
        },
        allowed_origins: match value("ui.allowed_origins") {
            Some(Value::Words(origins)) => origins,
            _ => Vec::new(),
        },
        log_level: match value("log.level") {
            Some(Value::Name(level)) => level,
            _ => "info".to_owned(),
        },
        log_format: match value("log.format") {
            Some(Value::Name(format)) => format,
            _ => "text".to_owned(),
        },
        log_ceiling: match value("log.ceiling") {
            Some(Value::Bytes(bytes)) => bytes,
            _ => DEFAULT_LOG_CEILING,
        },
    }
}

/// The settings a daemon starting now would run with: the global file's own table, or the defaults
/// where it has no valid version. Read without a [`Configuration`], so nothing is written — which is
/// how a report collected beside a daemon that is not running names, for instance, its log format.
pub fn starting_settings(global: Option<&GlobalConfig>) -> DaemonSettings {
    let layer = Layer::default();

    settings_of(&not_live(global.map_or(&layer, |global| &global.base)))
}

/// The keys that cannot change live, as `layer` alone sets them.
fn not_live(layer: &Layer) -> BTreeMap<&'static str, Setting> {
    merge(&[layer], Vec::new())
        .keys
        .into_iter()
        .filter(|key| !key.spec.live)
        .map(|key| (key.spec.name, key.setting))
        .collect()
}

/// Re-read the file at `path` into `tracked` if it changed, keeping the last known-good version in
/// force — and on disk at `known_good` — when the new one cannot be parsed.
fn refresh<T>(
    tracked: &mut Tracked<T>,
    path: &Path,
    known_good: &Path,
    parse: impl Fn(Option<&str>) -> Result<T, ConfigError>,
) {
    let text = match read_text(path) {
        Ok(text) => text,
        Err(error) => {
            tracked.seen = None;
            rejected(tracked, error, known_good, &parse);
            return;
        }
    };
    if tracked.seen.as_ref() == Some(&text) {
        return;
    }

    match parse(text.as_deref()) {
        Ok(parsed) => {
            tracked.in_force = Some(parsed);
            tracked.error = None;
            // A lost copy costs only the fallback after a restart, never the configuration in force,
            // so failing to keep one is not a reason to refuse a valid file — but it is said.
            // An absent file is kept as an empty one, which parses to the same nothing: the defaults
            // were a valid version too, and a restart must find that out rather than find no copy.
            let kept = write_atomically(known_good, text.as_deref().unwrap_or_default());
            if let Err(error) = kept {
                tracing::warn!(
                    file = %path.display(),
                    copy = %known_good.display(),
                    %error,
                    "the known-good copy of a configuration file could not be kept"
                );
            }
        }
        Err(error) => rejected(tracked, error, known_good, &parse),
    }
    tracked.seen = Some(text);
}

/// Record why the current version of a file is not applied, falling back to its copy on disk when no
/// version is in force yet — which is the case after a restart.
fn rejected<T>(
    tracked: &mut Tracked<T>,
    error: ConfigError,
    known_good: &Path,
    parse: &impl Fn(Option<&str>) -> Result<T, ConfigError>,
) {
    tracked.error = Some(error);
    if tracked.in_force.is_none() {
        tracked.in_force = recovered(known_good, parse);
    }
}

/// The known-good copy at `known_good`, parsed as the file it is a copy of, if there is one and it
/// still parses.
fn recovered<T>(
    known_good: &Path,
    parse: &impl Fn(Option<&str>) -> Result<T, ConfigError>,
) -> Option<T> {
    let text = fs::read_to_string(known_good).ok()?;
    parse(Some(&text)).ok()
}

/// Replace the file at `path` with `text`, never leaving it half-written.
fn write_atomically(path: &Path, text: &str) -> io::Result<()> {
    let temporary = path.with_extension("toml.new");
    let mut file = fs::File::create(&temporary)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);

    fs::rename(&temporary, path)
}

/// The text of the file at `path`, `None` when there is none.
fn read_text(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ConfigError {
            path: path.to_path_buf(),
            message: format!("it could not be read: {error}"),
        }),
    }
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

/// The 1-based line holding byte `offset` of `text`.
fn line_of(text: &str, offset: usize) -> u32 {
    let before = &text[..offset.min(text.len())];
    u32::try_from(before.matches('\n').count() + 1).unwrap_or(u32::MAX)
}

/// `path` with every link resolved, or as it is when it cannot be.
fn resolved(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Where the History Stores live, as a path inside the Root at `root_path` — when they are inside it, as
/// they are in a Root that is a whole home directory — so a capture withholds them as `history_store`
/// rather than capturing history into history, and the watcher never watches its own writes.
fn history_fence(state_dir: &Path, root_path: &Path) -> Option<(RootPath, WithheldReason)> {
    let state_dir = resolved(state_dir);
    let inside = state_dir.strip_prefix(resolved(root_path)).ok()?;

    let mut fenced = RootPath::root();
    for component in inside.components() {
        fenced = fenced.child(component.as_os_str().to_str()?);
    }

    (!fenced.is_root()).then_some((fenced, WithheldReason::HistoryStore))
}
