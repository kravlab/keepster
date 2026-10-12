//! A diagnostics bundle: the files `keepster diagnose` writes into one local zip, and the inventory it
//! prints before writing them (ticket 63, ADR 0013).
//!
//! **The content is fixed.** There are no selection flags, because a trimmed bundle looks exactly like a
//! complete one; a bundle collected without the daemon holds what the disk holds and names every section
//! it lacks, with why, so it can never pass for a whole one.
//!
//! **The user is the filter, and the inventory equips them to be one.** It lists every file, where it
//! came from and what it reveals, and it is printed, not buried: paths go in as they are, and nothing is
//! redacted — a redaction pass would promise what it cannot keep. A reader who finds no redaction code
//! here should not add it.
//!
//! **Nothing here sends anything anywhere.** The bundle is written to the one local file the caller
//! gives; there is no other destination.

use std::io::{self, Write};
use std::time::SystemTime;

use keepster_api::{
    DiagnosticConfiguration, Diagnostics, LogTail, RegistryRecord, RootFilesystem, VersionInfo,
};
use keepster_core::{ArchiveEntry, ArchiveWriter, Crc32};
use serde::Serialize;
use serde_json::json;

/// The name the inventory is held under in the archive.
pub const INVENTORY: &str = "inventory.txt";

/// The directory the log's files are held under in the archive.
const LOGS: &str = "logs/";

/// One file of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleFile {
    /// Its path in the archive.
    pub name: String,
    /// Where it came from.
    pub source: String,
    /// What it reveals, in one line, for the person deciding whether to share it.
    pub discloses: String,
    /// Its bytes.
    pub content: Vec<u8>,
}

/// A section a whole bundle holds and this one does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    /// The file, or the part of a file, that is not here.
    pub name: String,
    /// Why.
    pub reason: String,
}

/// What the disk alone gives a bundle when the daemon cannot be asked.
#[derive(Debug, Clone)]
pub struct OnDisk {
    /// The environment as far as this process can read it.
    pub environment: DiskEnvironment,
    /// The configuration as the files read now, which may differ from what a running daemon holds.
    pub configuration: DiagnosticConfiguration,
    /// Why no Root's configuration could be read, when the Registry names Roots and none could.
    pub unread_roots: Option<String>,
    /// The Registry, or why it could not be read.
    pub registry: Result<Vec<RegistryRecord>, String>,
    /// The tail of the log.
    pub log: LogTail,
}

/// The environment as a process other than the daemon can read it: the daemon's
/// [`Environment`](keepster_api::Environment) without what lives in the daemon's memory alone — the
/// watches in use, the `git` discovery result and the last Maintenance Passes — which the bundle names
/// missing. Its JSON is the daemon's with those fields absent, never filled with stand-ins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiskEnvironment {
    /// The operating system, as Rust names it.
    pub os: String,
    /// The processor architecture, as Rust names it.
    pub arch: String,
    /// The filesystem under each watched Root.
    pub filesystems: Vec<RootFilesystem>,
    /// The directory watches' ceiling.
    pub watches: DiskWatches,
}

/// The directory watches as a process other than the daemon can read them: the ceiling alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiskWatches {
    /// The most watches the daemon would hold together, where the platform has a per-user budget.
    pub ceiling: Option<u64>,
}

/// A bundle: its files, what it lacks, and how it was collected.
#[derive(Debug, Clone)]
pub struct Bundle {
    files: Vec<BundleFile>,
    missing: Vec<Missing>,
    collected_by: String,
    log_format: String,
}

impl Bundle {
    /// The bundle of what the daemon answered, beside this build's own versions as `cli`.
    pub fn from_daemon(cli: VersionInfo, found: Diagnostics) -> Self {
        let mut bundle = Self::new("the daemon, over its control channel", &found.log.format);
        bundle.add(
            "versions.json",
            "this keepster build and the daemon's status",
            DISCLOSES_VERSIONS,
            &json!({ "cli": cli, "daemon": found.status.daemon.version }),
        );
        bundle.add(
            "status.json",
            "the daemon: GET /v1/status",
            DISCLOSES_STATUS,
            &found.status,
        );
        bundle.add(
            "environment.json",
            "the daemon",
            DISCLOSES_ENVIRONMENT,
            &found.environment,
        );
        bundle.add(
            "configuration.json",
            "the daemon: the configuration it has in force",
            DISCLOSES_CONFIGURATION,
            &found.configuration,
        );
        bundle.add(
            "registry.json",
            "the daemon: the Registry",
            DISCLOSES_REGISTRY,
            &found.registry,
        );
        bundle.add(
            "topology.json",
            "the daemon: each Root's History Store",
            DISCLOSES_TOPOLOGY,
            &found.topology,
        );
        bundle.add(
            "watermarks.json",
            "the daemon: each Root's derived index",
            DISCLOSES_WATERMARKS,
            &found.watermarks,
        );
        bundle.add_log(
            &found.log,
            &found.log.format,
            "the daemon writes no log file",
        );

        bundle
    }

    /// The bundle of what the disk holds, collected because the daemon could not be asked — `why`.
    ///
    /// The log's format is read from the newest of its own lines: the configuration file may have been
    /// edited since the daemon started, and then names a format no line is in. Only a log with no line
    /// to read falls back on the file, and the inventory says which it was.
    pub fn from_disk(cli: VersionInfo, found: OnDisk, why: &str) -> Self {
        let written_in = found
            .log
            .files
            .first()
            .and_then(|file| format_of(&file.text));
        let log_format = match written_in {
            Some(format) => format!("{format}, read from the log's own lines"),
            None => format!("{}, as the configuration file reads now", found.log.format),
        };
        let mut bundle = Self::new(
            &format!("this command, from the files on disk, because {why}"),
            &log_format,
        );
        let missing = |name: &str| Missing {
            name: name.to_owned(),
            reason: format!("only the daemon has it, and {why}"),
        };

        bundle.add(
            "versions.json",
            "this keepster build",
            DISCLOSES_VERSIONS,
            &json!({ "cli": cli }),
        );
        bundle
            .missing
            .push(missing("versions.json (the daemon's versions)"));
        bundle.missing.push(missing("status.json"));

        bundle.add(
            "environment.json",
            "this command",
            DISCLOSES_ENVIRONMENT,
            &found.environment,
        );
        bundle.missing.push(missing(
            "environment.json (the watches in use, the git discovery result, the last \
             maintenance passes)",
        ));

        bundle.add(
            "configuration.json",
            "this command: the configuration files as they read now",
            DISCLOSES_CONFIGURATION,
            &found.configuration,
        );
        if let Some(reason) = &found.unread_roots {
            bundle.missing.push(Missing {
                name: "configuration.json (each Root's configuration)".to_owned(),
                reason: reason.clone(),
            });
        }
        match &found.registry {
            Ok(registry) => bundle.add(
                "registry.json",
                "this command: the Registry on disk",
                DISCLOSES_REGISTRY,
                registry,
            ),
            Err(error) => bundle.missing.push(Missing {
                name: "registry.json".to_owned(),
                reason: format!("the Registry could not be read: {error}"),
            }),
        }
        bundle.missing.push(missing("topology.json"));
        bundle.missing.push(missing("watermarks.json"));
        bundle.add_log(
            &found.log,
            written_in.unwrap_or(&found.log.format),
            "no log file was found",
        );

        bundle
    }

    /// Every file, in the order the archive holds them after the inventory.
    pub fn files(&self) -> &[BundleFile] {
        &self.files
    }

    /// Every section this bundle lacks.
    pub fn missing(&self) -> &[Missing] {
        &self.missing
    }

    /// The inventory: every file, where it came from and what it reveals, and every section missing —
    /// printed before the archive is written, and held in it as [`INVENTORY`].
    pub fn inventory(&self) -> String {
        let mut text = String::new();
        text.push_str("Keepster diagnostics bundle\n\n");
        text.push_str(
            "Nothing in it has been sent anywhere, and nothing will be: it is one local file.\n\
             Paths are included as they are, and nothing is redacted, so read this list before\n\
             you share it. No byte of any file from your history is in it.\n\n",
        );
        text.push_str(&format!("Collected by: {}\n", self.collected_by));
        text.push_str(&format!("Log format: {}\n\n", self.log_format));

        text.push_str(&format!(
            "{INVENTORY}\n  from: this command\n  discloses: this list\n"
        ));
        for file in &self.files {
            text.push_str(&format!(
                "{} ({} bytes)\n  from: {}\n  discloses: {}\n",
                file.name,
                file.content.len(),
                file.source,
                file.discloses
            ));
        }

        if !self.missing.is_empty() {
            text.push_str("\nMissing from this bundle:\n");
            for missing in &self.missing {
                text.push_str(&format!("  {}: {}\n", missing.name, missing.reason));
            }
        }

        text
    }

    /// Write the archive into `out`: the inventory first, then every file, each entry stamped `moment`.
    ///
    /// Stored, not compressed, as Export's archive is: the bundle is small, and a person can read it
    /// without unpacking it.
    pub fn write_zip(&self, out: &mut dyn Write, moment: SystemTime) -> io::Result<()> {
        let inventory = self.inventory();
        let mut writer = ArchiveWriter::new(moment);
        let entries = std::iter::once((INVENTORY, inventory.as_bytes())).chain(
            self.files
                .iter()
                .map(|file| (file.name.as_str(), file.content.as_slice())),
        );
        for (name, content) in entries {
            let entry = ArchiveEntry {
                name: name.to_owned(),
                size: content.len() as u64,
                mode: Some(0o600),
                executable: false,
                link: false,
            };
            out.write_all(&writer.begin(&entry))?;
            out.write_all(content)?;
            let mut crc = Crc32::new();
            crc.update(content);
            out.write_all(&writer.end(&entry, crc.finish()))?;
        }
        out.write_all(&writer.finish())?;

        out.flush()
    }

    /// An empty bundle, collected by `collected_by`, its log in `log_format` as the inventory states it.
    fn new(collected_by: &str, log_format: &str) -> Self {
        Self {
            files: Vec::new(),
            missing: Vec::new(),
            collected_by: collected_by.to_owned(),
            log_format: log_format.to_owned(),
        }
    }

    /// Add `value` as the JSON file `name`.
    fn add(&mut self, name: &str, source: &str, discloses: &str, value: &impl Serialize) {
        let mut content = serde_json::to_vec_pretty(value).expect("a wire type serializes");
        content.push(b'\n');
        self.files.push(BundleFile {
            name: name.to_owned(),
            source: source.to_owned(),
            discloses: discloses.to_owned(),
            content,
        });
    }

    /// Add every file of the log's tail, its lines in `format`, or say there is none: because the log
    /// could not be read, when that is the reason, and otherwise `why`.
    fn add_log(&mut self, log: &LogTail, format: &str, why: &str) {
        if log.files.is_empty() {
            self.missing.push(Missing {
                name: LOGS.to_owned(),
                reason: match &log.error {
                    Some(error) => format!("the log could not be read: {error}"),
                    None => why.to_owned(),
                },
            });
        }
        for file in &log.files {
            let source = if file.truncated {
                format!(
                    "the daemon's log: the last {} of its {} bytes, whole lines only",
                    file.text.len(),
                    file.size
                )
            } else {
                "the daemon's log, the whole file".to_owned()
            };
            self.files.push(BundleFile {
                name: format!("{LOGS}{}", file.name),
                source,
                discloses: format!("{DISCLOSES_LOG} (lines in the {format} format)"),
                content: file.text.clone().into_bytes(),
            });
        }
    }
}

/// The format `text`'s lines are written in, from its first line: a `json` line is an object, and a
/// `text` line starts with its timestamp. `None` for a text with no line.
fn format_of(text: &str) -> Option<&'static str> {
    let first = text.lines().find(|line| !line.trim().is_empty())?;

    Some(if first.starts_with('{') {
        "json"
    } else {
        "text"
    })
}

const DISCLOSES_VERSIONS: &str =
    "the build, API and store-format versions of this command and of the daemon; no paths";
const DISCLOSES_STATUS: &str = "every Root's path and state, the daemon's process id and uptime, \
     the path of its log and of the git it found";
const DISCLOSES_ENVIRONMENT: &str = "the operating system, each Root's path, filesystem and mount \
     point, the directory watches against their ceiling, the git found, and the last maintenance \
     passes' error output, which may name paths";
const DISCLOSES_CONFIGURATION: &str = "every configuration key with its value and the file and line \
     that set it: your include and exclude patterns, the configuration files' paths, each Root's path";
const DISCLOSES_REGISTRY: &str = "every Root's identity and path, watched or not, and the identity \
     of the volume it was registered on";
const DISCLOSES_TOPOLOGY: &str = "per Root: Segment and Pin names, commit and Snapshot identities, \
     when each Snapshot was taken, counts and sizes on disk; no file names and no file content";
const DISCLOSES_WATERMARKS: &str = "per Root: the derived index's sources and the commits it caught \
     up to; no file names and no file content";
const DISCLOSES_LOG: &str = "the daemon's own lines: paths it captured, skipped or could not read, \
     error messages, Root paths";
