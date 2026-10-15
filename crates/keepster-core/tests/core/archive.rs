//! The archive a selection downloads as (ticket 58): zip64, stored, its exact size known before the first
//! byte — read back here by the central directory, as `unzip` and Explorer read it.

use std::time::{Duration, UNIX_EPOCH};

use keepster_core::{ArchiveEntry, ArchiveWriter, Crc32, archive_size};

fn entry(
    name: impl AsRef<[u8]>,
    content: &[u8],
    mode: Option<u32>,
    link: bool,
) -> (ArchiveEntry, Vec<u8>) {
    (
        ArchiveEntry {
            name: name.as_ref().to_vec(),
            size: content.len() as u64,
            mode,
            executable: false,
            link,
        },
        content.to_vec(),
    )
}

/// The archive of `files`, written as the daemon streams it.
fn archive(files: &[(ArchiveEntry, Vec<u8>)]) -> Vec<u8> {
    // 2026-10-12 10:20:30 UTC.
    let moment = UNIX_EPOCH + Duration::from_secs(1_791_800_430);
    let mut writer = ArchiveWriter::new(moment);
    let mut bytes = Vec::new();
    for (entry, content) in files {
        bytes.extend(writer.begin(entry));
        bytes.extend_from_slice(content);
        let mut crc = Crc32::new();
        crc.update(content);
        bytes.extend(writer.end(entry, crc.finish()));
    }
    bytes.extend(writer.finish());
    bytes
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().expect("two bytes"))
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

/// One file as the central directory describes it, and its bytes.
#[derive(Debug, PartialEq, Eq)]
struct Read {
    name: Vec<u8>,
    /// Whether the name is declared UTF-8 (bit 11), in the central and the local header alike.
    utf8: bool,
    unix_mode: u32,
    content: Vec<u8>,
}

/// Every file of `bytes`, found from its end as a reader finds them, each CRC checked.
fn read(bytes: &[u8]) -> Vec<Read> {
    let end = bytes.len() - 22;
    assert_eq!(u32_at(bytes, end), 0x0605_4b50, "the end record");
    let locator = end - 20;
    assert_eq!(u32_at(bytes, locator), 0x0706_4b50, "the zip64 locator");
    let zip64_end = u64_at(bytes, locator + 8) as usize;
    assert_eq!(
        u32_at(bytes, zip64_end),
        0x0606_4b50,
        "the zip64 end record"
    );
    let count = u64_at(bytes, zip64_end + 32);
    let mut at = u64_at(bytes, zip64_end + 48) as usize;

    let mut found = Vec::new();
    for _ in 0..count {
        assert_eq!(u32_at(bytes, at), 0x0201_4b50, "a central header");
        let utf8 = u16_at(bytes, at + 8) & 0x0800 != 0;
        let crc = u32_at(bytes, at + 16);
        let name_length = u16_at(bytes, at + 28) as usize;
        let extra_length = u16_at(bytes, at + 30) as usize;
        let unix_mode = u32_at(bytes, at + 38) >> 16;
        let name = bytes[at + 46..at + 46 + name_length].to_vec();
        let extra = at + 46 + name_length;
        assert_eq!(u16_at(bytes, extra), 0x0001, "a zip64 field");
        let size = u64_at(bytes, extra + 4) as usize;
        let offset = u64_at(bytes, extra + 20) as usize;

        assert_eq!(u32_at(bytes, offset), 0x0403_4b50, "a local header");
        assert_eq!(
            u16_at(bytes, offset + 6) & 0x0800 != 0,
            utf8,
            "both headers say the same of the name"
        );
        let local_name = u16_at(bytes, offset + 26) as usize;
        let local_extra = u16_at(bytes, offset + 28) as usize;
        let data = offset + 30 + local_name + local_extra;
        let content = bytes[data..data + size].to_vec();
        let mut computed = Crc32::new();
        computed.update(&content);
        assert_eq!(
            computed.finish(),
            crc,
            "{}'s CRC",
            String::from_utf8_lossy(&name)
        );
        assert_eq!(u32_at(bytes, data + size), 0x0807_4b50, "a data descriptor");
        assert_eq!(u32_at(bytes, data + size + 4), crc);

        found.push(Read {
            name,
            utf8,
            unix_mode,
            content,
        });
        at = extra + extra_length;
    }

    found
}

#[test]
fn the_crc_is_the_one_zip_uses() {
    let mut crc = Crc32::new();
    crc.update(b"1234");
    crc.update(b"56789");
    assert_eq!(crc.finish(), 0xCBF4_3926);
    assert_eq!(Crc32::new().finish(), 0);
}

#[test]
fn an_archive_is_exactly_the_size_promised_and_reads_back_whole() {
    let files = [
        entry("notes.txt", b"kept\n", Some(0o600), false),
        entry("bin/run.sh", b"#!/bin/sh\necho hi\n", Some(0o755), false),
        entry("caf\u{e9}/menu.txt", b"", None, false),
        entry("latest", b"notes.txt", None, true),
    ];
    let entries: Vec<ArchiveEntry> = files.iter().map(|(entry, _)| entry.clone()).collect();

    let bytes = archive(&files);

    assert_eq!(
        bytes.len() as u64,
        archive_size(&entries),
        "the size shown before the download is the size sent"
    );
    let found = read(&bytes);
    assert_eq!(
        found,
        [
            Read {
                name: "notes.txt".as_bytes().to_vec(),
                utf8: true,
                unix_mode: 0o100_600,
                content: b"kept\n".to_vec(),
            },
            Read {
                name: "bin/run.sh".as_bytes().to_vec(),
                utf8: true,
                unix_mode: 0o100_755,
                content: b"#!/bin/sh\necho hi\n".to_vec(),
            },
            Read {
                name: "caf\u{e9}/menu.txt".as_bytes().to_vec(),
                utf8: true,
                unix_mode: 0o100_644,
                content: Vec::new(),
            },
            Read {
                name: "latest".as_bytes().to_vec(),
                utf8: true,
                unix_mode: 0o120_777,
                content: b"notes.txt".to_vec(),
            },
        ]
    );
}

#[test]
fn an_empty_selection_is_a_valid_empty_archive() {
    let bytes = archive(&[]);

    assert_eq!(bytes.len() as u64, archive_size(&[]));
    assert!(read(&bytes).is_empty());
}

#[test]
fn the_same_selection_is_always_the_same_bytes() {
    let files = [entry("a.txt", b"a", None, false)];

    assert_eq!(archive(&files), archive(&files));
}

#[test]
fn a_file_past_four_gibibytes_needs_nothing_special() {
    let huge = ArchiveEntry {
        name: b"disk.img".to_vec(),
        size: 5 * 1024 * 1024 * 1024,
        mode: None,
        executable: false,
        link: false,
    };
    let mut writer = ArchiveWriter::new(UNIX_EPOCH);

    let header = writer.begin(&huge);
    let descriptor = writer.end(&huge, 0);

    assert_eq!(u64_at(&header, 30 + 8 + 4), huge.size, "the zip64 size");
    assert_eq!(u64_at(&descriptor, 8), huge.size);
    assert!(archive_size(std::slice::from_ref(&huge)) > huge.size);
}

#[test]
fn a_name_that_is_not_utf8_is_written_as_its_bytes_and_not_declared_utf8() {
    let files = [
        entry(b"src/\xff.bin", b"odd", None, false),
        entry("notes.txt", b"kept", None, false),
    ];

    let found = read(&archive(&files));

    assert_eq!(
        found
            .iter()
            .map(|read| (read.name.as_slice(), read.utf8))
            .collect::<Vec<_>>(),
        [(&b"src/\xff.bin"[..], false), (&b"notes.txt"[..], true)],
        "an unzip that honours bit 11 would misread the bytes as UTF-8"
    );
}
