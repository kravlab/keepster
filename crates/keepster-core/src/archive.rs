//! The archive a selection of a Snapshot downloads as (ticket 58, ticket 19): a zip64 file whose every
//! entry is stored, not compressed, so that its exact size is known before the first byte is sent — what
//! a browser shows beside the download, and what `Content-Length` promises.
//!
//! Every entry carries zip64 sizes whatever its size, so that a file past 4 GiB needs nothing special, and
//! a data descriptor after its bytes, since the CRC is known only once they have streamed past. Names are
//! `/`-separated bytes, declared UTF-8 where they are and written as they are where they are not (ticket
//! 74), and permission bits travel in the external attributes, as `unzip` reads them on Unix; Explorer
//! ignores them. Only the byte layout is here: the bytes of each file are the caller's.

use std::time::SystemTime;

/// One file of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveEntry {
    /// Its path inside the archive, `/`-separated, as the bytes the Snapshot holds.
    pub name: Vec<u8>,
    /// How many bytes it holds — for a link, its target's text.
    pub size: u64,
    /// Its permission bits, where the Snapshot observed them.
    pub mode: Option<u32>,
    /// Whether its tree mode is executable: all a Snapshot that observed no modes has to give.
    pub executable: bool,
    /// Whether it is a symbolic link, its bytes the target.
    pub link: bool,
}

/// A local header, without its name: signature, version, flags, method, time, date, CRC, two sizes and
/// two lengths.
const LOCAL_HEADER: u64 = 30;
/// The zip64 field of a local header: its tag and length, and both sizes.
const LOCAL_ZIP64: u64 = 20;
/// The data descriptor after an entry's bytes: signature, CRC and both sizes, eight bytes each.
const DESCRIPTOR: u64 = 24;
/// A central directory header, without its name.
const CENTRAL_HEADER: u64 = 46;
/// The zip64 field of a central header: its tag and length, both sizes and the local header's offset.
const CENTRAL_ZIP64: u64 = 28;
/// The zip64 end record, its locator and the classic end record.
const END: u64 = 56 + 20 + 22;

/// Version 4.5, the first with zip64.
const VERSION: u16 = 45;
/// Made on Unix, so the external attributes hold permission bits.
const MADE_BY: u16 = (3 << 8) | VERSION;
/// The CRC and sizes follow the bytes (bit 3).
const DESCRIPTOR_FOLLOWS: u16 = 0x0008;
/// The name is UTF-8 (bit 11).
const UTF8_NAME: u16 = 0x0800;
/// What a 32-bit field holds when its value is in the zip64 field.
const IN_ZIP64: u32 = u32::MAX;

/// The exact size of the archive of `entries`.
pub fn archive_size(entries: &[ArchiveEntry]) -> u64 {
    entries
        .iter()
        .map(|entry| {
            let name = entry.name.len() as u64;
            LOCAL_HEADER
                + LOCAL_ZIP64
                + name
                + entry.size
                + DESCRIPTOR
                + CENTRAL_HEADER
                + CENTRAL_ZIP64
                + name
        })
        .sum::<u64>()
        + END
}

/// An archive being written: the bytes around each entry's own, handed out as they are due.
#[derive(Debug, Clone)]
pub struct ArchiveWriter {
    /// How many bytes have been handed out so far, theirs included.
    written: u64,
    /// Every entry's central directory header, written last.
    central: Vec<u8>,
    /// How many entries.
    entries: u64,
    /// The modification time every entry carries, as MS-DOS spells it.
    time: u16,
    /// Its date.
    date: u16,
}

impl ArchiveWriter {
    /// An archive whose every entry was last modified at `moment`: the Snapshot's own moment, so the
    /// same selection of the same Snapshot is always the same bytes.
    pub fn new(moment: SystemTime) -> Self {
        let (time, date) = dos_moment(moment);
        Self {
            written: 0,
            central: Vec::new(),
            entries: 0,
            time,
            date,
        }
    }

    /// The bytes before `entry`'s own.
    pub fn begin(&mut self, entry: &ArchiveEntry) -> Vec<u8> {
        let mut header =
            Vec::with_capacity((LOCAL_HEADER + LOCAL_ZIP64) as usize + entry.name.len());
        put32(&mut header, 0x0403_4b50);
        put16(&mut header, VERSION);
        put16(&mut header, flags(entry));
        put16(&mut header, 0);
        put16(&mut header, self.time);
        put16(&mut header, self.date);
        put32(&mut header, 0);
        put32(&mut header, IN_ZIP64);
        put32(&mut header, IN_ZIP64);
        put16(&mut header, name_length(entry));
        put16(&mut header, LOCAL_ZIP64 as u16);
        header.extend_from_slice(&entry.name);
        put16(&mut header, 0x0001);
        put16(&mut header, (LOCAL_ZIP64 - 4) as u16);
        put64(&mut header, entry.size);
        put64(&mut header, entry.size);

        let offset = self.written;
        self.written += header.len() as u64;
        self.central_header(entry, offset);
        header
    }

    /// The bytes after `entry`'s own, which were `entry.size` bytes whose CRC is `crc`.
    pub fn end(&mut self, entry: &ArchiveEntry, crc: u32) -> Vec<u8> {
        let mut descriptor = Vec::with_capacity(DESCRIPTOR as usize);
        put32(&mut descriptor, 0x0807_4b50);
        put32(&mut descriptor, crc);
        put64(&mut descriptor, entry.size);
        put64(&mut descriptor, entry.size);
        // The CRC belongs in the central header too, where it was left for now.
        let at =
            self.central.len() - (CENTRAL_HEADER + CENTRAL_ZIP64) as usize - entry.name.len() + 16;
        self.central[at..at + 4].copy_from_slice(&crc.to_le_bytes());

        self.written += entry.size + DESCRIPTOR;
        descriptor
    }

    /// The bytes after the last entry's: the central directory and the records that find it.
    pub fn finish(self) -> Vec<u8> {
        let central_at = self.written;
        let central_size = self.central.len() as u64;
        let mut end = self.central;

        let zip64_end_at = central_at + central_size;
        put32(&mut end, 0x0606_4b50);
        put64(&mut end, 44);
        put16(&mut end, MADE_BY);
        put16(&mut end, VERSION);
        put32(&mut end, 0);
        put32(&mut end, 0);
        put64(&mut end, self.entries);
        put64(&mut end, self.entries);
        put64(&mut end, central_size);
        put64(&mut end, central_at);

        put32(&mut end, 0x0706_4b50);
        put32(&mut end, 0);
        put64(&mut end, zip64_end_at);
        put32(&mut end, 1);

        put32(&mut end, 0x0605_4b50);
        put16(&mut end, 0);
        put16(&mut end, 0);
        put16(&mut end, u16::MAX);
        put16(&mut end, u16::MAX);
        put32(&mut end, IN_ZIP64);
        put32(&mut end, IN_ZIP64);
        put16(&mut end, 0);

        end
    }

    /// Note `entry`'s central directory header, its CRC still to come.
    fn central_header(&mut self, entry: &ArchiveEntry, offset: u64) {
        let central = &mut self.central;
        put32(central, 0x0201_4b50);
        put16(central, MADE_BY);
        put16(central, VERSION);
        put16(central, flags(entry));
        put16(central, 0);
        put16(central, self.time);
        put16(central, self.date);
        put32(central, 0);
        put32(central, IN_ZIP64);
        put32(central, IN_ZIP64);
        put16(central, name_length(entry));
        put16(central, CENTRAL_ZIP64 as u16);
        put16(central, 0);
        put16(central, 0);
        put16(central, 0);
        put32(central, unix_mode(entry) << 16);
        put32(central, IN_ZIP64);
        central.extend_from_slice(&entry.name);
        put16(central, 0x0001);
        put16(central, (CENTRAL_ZIP64 - 4) as u16);
        put64(central, entry.size);
        put64(central, entry.size);
        put64(central, offset);

        self.entries += 1;
    }
}

/// An entry's general-purpose flags. A name that is not UTF-8 is not declared so: an unzip honouring
/// bit 11 would decode its bytes as UTF-8, and one that does not reads them as they are.
fn flags(entry: &ArchiveEntry) -> u16 {
    if std::str::from_utf8(&entry.name).is_ok() {
        DESCRIPTOR_FOLLOWS | UTF8_NAME
    } else {
        DESCRIPTOR_FOLLOWS
    }
}

/// A name's length as its header field holds it; a Root-relative path is never near 64 KiB.
fn name_length(entry: &ArchiveEntry) -> u16 {
    u16::try_from(entry.name.len()).unwrap_or(u16::MAX)
}

/// The file type and permission bits `unzip` gives the entry on Unix.
fn unix_mode(entry: &ArchiveEntry) -> u32 {
    if entry.link {
        return 0o120_777;
    }
    let bits = match entry.mode {
        Some(mode) => mode & 0o7777,
        None if entry.executable => 0o755,
        None => 0o644,
    };

    0o100_000 | bits
}

/// `moment` as MS-DOS spells a time and a date, in UTC, held to the years it can spell.
fn dos_moment(moment: SystemTime) -> (u16, u16) {
    let Ok(timestamp) = jiff::Timestamp::try_from(moment) else {
        return (0, (1 << 5) | 1);
    };
    let utc = timestamp.to_zoned(jiff::tz::TimeZone::UTC);
    let year = utc.year().clamp(1980, 2107);
    let time =
        ((utc.hour() as u16) << 11) | ((utc.minute() as u16) << 5) | (utc.second() as u16 / 2);
    let date = (((year - 1980) as u16) << 9) | ((utc.month() as u16) << 5) | utc.day() as u16;

    (time, date)
}

/// `value` after `bytes`, little-endian, as every zip field is.
fn put16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// `value` after `bytes`, little-endian.
fn put32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// `value` after `bytes`, little-endian.
fn put64(bytes: &mut Vec<u8>, value: u64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

/// The CRC-32 a zip entry carries, computed as its bytes stream past.
#[derive(Debug, Clone, Copy)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    /// Nothing seen yet.
    pub fn new() -> Self {
        Self(u32::MAX)
    }

    /// `bytes` seen next.
    pub fn update(&mut self, bytes: &[u8]) {
        let mut crc = self.0;
        for byte in bytes {
            crc = CRC_TABLE[((crc ^ u32::from(*byte)) & 0xff) as usize] ^ (crc >> 8);
        }
        self.0 = crc;
    }

    /// The CRC of everything seen.
    pub fn finish(self) -> u32 {
        !self.0
    }
}

/// The reflected IEEE polynomial's table, one entry per byte value.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0_u32; 256];
    let mut value = 0;
    while value < 256 {
        let mut crc = value as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 == 1 {
                0xEDB8_8320 ^ (crc >> 1)
            } else {
                crc >> 1
            };
            bit += 1;
        }
        table[value] = crc;
        value += 1;
    }
    table
};
