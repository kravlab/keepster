//! The metadata sidecar: one blob at a Snapshot's tree root, the single home for every per-path fact git
//! cannot express (ADR 0006, ticket 48).
//!
//! It is line-oriented: a header naming the grammar's version and the Root's modal defaults, then one typed
//! record per line, `<type> <field> <path>`. Records are sorted by path, then type, so a sidecar holding the
//! same facts is the same bytes — the same blob — and an unchanged tree stays unchanged. The record alphabet
//! only grows: a record this build does not know is skipped and reported, never misread (ADR 0010).

use crate::path::RootPath;

/// The sidecar's name at a Snapshot's tree root. A file of that name at the top of a Working Tree is
/// Withheld `reserved_name`, never captured over it.
pub const SIDECAR: &str = ".keepster-meta";

/// The grammar this build writes and reads.
pub const SIDECAR_VERSION: u32 = 1;

/// What the first line says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// The grammar's version.
    pub version: u32,
    /// The mode most files of the Root have; ticket 50 computes it, and records only what differs.
    pub file_mode: u32,
    /// The mode most directories of the Root have.
    pub dir_mode: u32,
}

impl Default for Header {
    fn default() -> Self {
        Self {
            version: SIDECAR_VERSION,
            file_mode: 0o644,
            dir_mode: 0o755,
        }
    }
}

/// Why a Snapshot holds no content for a path that is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WithheldReason {
    /// Over its size cap.
    TooLarge,
    /// A pipe, a socket or a device: never read, so nothing can block a capture on it.
    SpecialFile(SpecialKind),
    /// A name the store keeps for itself: the watched directory's own `.git` and its aliases, and the
    /// sidecar's own name at the top.
    ReservedName,
    /// A name the store cannot hold (ticket 74).
    UnrepresentablePath,
    /// Another Watched Root's directory, which that Root captures.
    OtherRoot,
    /// A History Store's directory: history is not captured into history.
    HistoryStore,
    /// A file only in the cloud, which a capture never downloads (ticket 75).
    CloudPlaceholder,
}

impl WithheldReason {
    /// The reason's wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::SpecialFile(_) => "special_file",
            Self::ReservedName => "reserved_name",
            Self::UnrepresentablePath => "unrepresentable_path",
            Self::OtherRoot => "other_root",
            Self::HistoryStore => "history_store",
            Self::CloudPlaceholder => "cloud_placeholder",
        }
    }

    /// What kind of special file, for `special_file`.
    pub fn kind(self) -> Option<SpecialKind> {
        match self {
            Self::SpecialFile(kind) => Some(kind),
            _ => None,
        }
    }

    /// The reason a record's field spells — `special_file:<kind>` for a special file.
    fn field(self) -> String {
        match self {
            Self::SpecialFile(kind) => format!("special_file:{}", kind.as_str()),
            other => other.as_str().to_owned(),
        }
    }

    fn parse_field(field: &str) -> Option<Self> {
        if let Some(kind) = field.strip_prefix("special_file:") {
            return SpecialKind::parse(kind).map(Self::SpecialFile);
        }

        Some(match field {
            "too_large" => Self::TooLarge,
            "reserved_name" => Self::ReservedName,
            "unrepresentable_path" => Self::UnrepresentablePath,
            "other_root" => Self::OtherRoot,
            "history_store" => Self::HistoryStore,
            "cloud_placeholder" => Self::CloudPlaceholder,
            _ => return None,
        })
    }
}

/// What a special file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialKind {
    /// A named pipe.
    Fifo,
    /// A Unix domain socket.
    Socket,
    /// A character device.
    CharDevice,
    /// A block device.
    BlockDevice,
}

impl SpecialKind {
    /// The kind's wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fifo => "fifo",
            Self::Socket => "socket",
            Self::CharDevice => "char_device",
            Self::BlockDevice => "block_device",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [
            Self::Fifo,
            Self::Socket,
            Self::CharDevice,
            Self::BlockDevice,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == name)
    }
}

/// Why a file meant to be captured is not in a Snapshot as it is now — which makes the Snapshot
/// incomplete.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Incomplete {
    /// It kept changing while it was read; its previous version, if any, is carried forward.
    Unstable,
    /// Reading it failed, with this error's symbolic name: `eacces` on Unix, `sharing_violation` on
    /// Windows.
    Error(String),
}

impl Incomplete {
    /// The reason's wire name: `unstable`, or the error's.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Unstable => "unstable",
            Self::Error(name) => name,
        }
    }
}

/// One fact about one path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Record {
    /// `w`: the path is there, and its content deliberately is not held.
    Withheld {
        /// Where.
        path: RootPath,
        /// Why.
        reason: WithheldReason,
    },
    /// `i`: the path was meant to be captured and could not be as it is now.
    Incomplete {
        /// Where.
        path: RootPath,
        /// Why.
        why: Incomplete,
    },
}

impl Record {
    /// The path the record is about.
    pub fn path(&self) -> &RootPath {
        match self {
            Self::Withheld { path, .. } | Self::Incomplete { path, .. } => path,
        }
    }

    fn letter(&self) -> char {
        match self {
            Self::Withheld { .. } => 'w',
            Self::Incomplete { .. } => 'i',
        }
    }

    fn field(&self) -> String {
        match self {
            Self::Withheld { reason, .. } => reason.field(),
            Self::Incomplete { why, .. } => why.as_str().to_owned(),
        }
    }
}

/// A sidecar's content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sidecar {
    /// The first line.
    pub header: Header,
    /// The records, in the order they are written: by path, then type.
    pub records: Vec<Record>,
}

/// A sidecar read back, with what this build could not understand of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// What was understood.
    pub sidecar: Sidecar,
    /// Every record line skipped because this build does not know its type or its field — to be
    /// reported, since a later build wrote facts this one cannot show.
    pub unknown: Vec<String>,
}

/// Why a blob is not a sidecar this build can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SidecarError {
    /// The first line is not a sidecar header.
    #[error("the metadata sidecar has no header this build recognises")]
    Header,
    /// A later grammar wrote it.
    #[error("the metadata sidecar is version {0}, newer than this build reads")]
    Version(u32),
}

impl Sidecar {
    /// A sidecar with the default header holding `records`, put in the order they are written.
    pub fn with_records(mut records: Vec<Record>) -> Self {
        sort(&mut records);

        Self {
            header: Header::default(),
            records,
        }
    }

    /// The blob's bytes. The same facts always give the same bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut records = self.records.clone();
        sort(&mut records);

        let mut text = format!(
            "keepster-meta {} file={:04o} dir={:04o}\n",
            self.header.version, self.header.file_mode, self.header.dir_mode
        );
        for record in &records {
            text.push(record.letter());
            text.push(' ');
            text.push_str(&record.field());
            text.push(' ');
            text.push_str(&quote(record.path().as_str()));
            text.push('\n');
        }

        text.into_bytes()
    }

    /// Read a sidecar blob.
    pub fn parse(bytes: &[u8]) -> Result<Parsed, SidecarError> {
        let text = String::from_utf8_lossy(bytes);
        let mut lines = text.lines();
        let header = parse_header(lines.next().ok_or(SidecarError::Header)?)?;

        let mut records = Vec::new();
        let mut unknown = Vec::new();
        for line in lines.filter(|line| !line.is_empty()) {
            match parse_record(line) {
                Some(record) => records.push(record),
                None => unknown.push(line.to_owned()),
            }
        }

        Ok(Parsed {
            sidecar: Sidecar { header, records },
            unknown,
        })
    }
}

fn sort(records: &mut [Record]) {
    records.sort_by(|a, b| {
        a.path()
            .as_str()
            .as_bytes()
            .cmp(b.path().as_str().as_bytes())
            .then(a.letter().cmp(&b.letter()))
    });
}

fn parse_header(line: &str) -> Result<Header, SidecarError> {
    let mut words = line.split(' ');
    if words.next() != Some("keepster-meta") {
        return Err(SidecarError::Header);
    }
    let version: u32 = words
        .next()
        .and_then(|version| version.parse().ok())
        .ok_or(SidecarError::Header)?;
    if version > SIDECAR_VERSION {
        return Err(SidecarError::Version(version));
    }

    let mut header = Header {
        version,
        ..Header::default()
    };
    for word in words {
        let mode = |text: &str| u32::from_str_radix(text, 8).ok();
        if let Some(found) = word.strip_prefix("file=").and_then(mode) {
            header.file_mode = found;
        } else if let Some(found) = word.strip_prefix("dir=").and_then(mode) {
            header.dir_mode = found;
        }
    }

    Ok(header)
}

/// One record line, or `None` for one this build does not know.
fn parse_record(line: &str) -> Option<Record> {
    let (letter, rest) = line.split_once(' ')?;
    let (field, path) = rest.split_once(' ')?;
    let path = RootPath::parse(&unquote(path)?).ok()?;

    match letter {
        "w" => Some(Record::Withheld {
            path,
            reason: WithheldReason::parse_field(field)?,
        }),
        "i" => Some(Record::Incomplete {
            path,
            why: match field {
                "unstable" => Incomplete::Unstable,
                name if !name.is_empty() => Incomplete::Error(name.to_owned()),
                _ => return None,
            },
        }),
        _ => None,
    }
}

/// A path as one line holds it: as it is, or quoted as git quotes a name it cannot print plainly.
fn quote(path: &str) -> String {
    let plain = !path.starts_with([' ', '"'])
        && !path
            .chars()
            .any(|character| character == '"' || character == '\\' || character.is_control());
    if plain {
        return path.to_owned();
    }

    let mut quoted = String::from("\"");
    for character in path.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            // Each byte of its UTF-8 in octal, as git escapes a byte it will not print.
            control if control.is_control() => {
                for byte in control.encode_utf8(&mut [0; 4]).bytes() {
                    quoted.push_str(&format!("\\{byte:03o}"));
                }
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');

    quoted
}

/// The path a field holds, unquoting a quoted one; `None` for a quoted one that is malformed.
fn unquote(field: &str) -> Option<String> {
    let Some(inner) = field.strip_prefix('"') else {
        return Some(field.to_owned());
    };
    let inner = inner.strip_suffix('"')?;

    // Bytes, because an octal escape is one byte of a name, not one character.
    let mut path = Vec::new();
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            path.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
            continue;
        }
        match characters.next()? {
            'n' => path.push(b'\n'),
            't' => path.push(b'\t'),
            'r' => path.push(b'\r'),
            '"' => path.push(b'"'),
            '\\' => path.push(b'\\'),
            first @ '0'..='7' => {
                let digits: String = std::iter::once(first)
                    .chain(characters.by_ref().take(2))
                    .collect();
                path.push(u8::from_str_radix(&digits, 8).ok()?);
            }
            _ => return None,
        }
    }

    String::from_utf8(path).ok()
}
