//! Recognising a cloud placeholder from what a walk's `stat` already holds (ticket 75): Windows' file
//! attributes and macOS's `st_flags`, read without opening the file.

use keepster_core::{is_cloud_placeholder, placeholder_by_attributes, placeholder_by_flags};

// FILE_ATTRIBUTE_* and SF_* as the platforms' headers define them.
const READONLY: u32 = 0x1;
const ARCHIVE: u32 = 0x20;
const REPARSE_POINT: u32 = 0x400;
const OFFLINE: u32 = 0x1000;
const RECALL_ON_OPEN: u32 = 0x4_0000;
const PINNED: u32 = 0x8_0000;
const UNPINNED: u32 = 0x10_0000;
const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
const UF_HIDDEN: u32 = 0x8000;
const SF_IMMUTABLE: u32 = 0x2_0000;
const SF_DATALESS: u32 = 0x4000_0000;

#[test]
fn a_windows_file_whose_data_is_not_all_local_is_a_placeholder() {
    for attributes in [
        RECALL_ON_DATA_ACCESS,
        RECALL_ON_OPEN,
        OFFLINE,
        // A cloud file freed from the disk carries its reparse point and is unpinned too.
        RECALL_ON_DATA_ACCESS | REPARSE_POINT | UNPINNED | ARCHIVE,
    ] {
        assert!(placeholder_by_attributes(attributes), "{attributes:#x}");
    }
}

#[test]
fn a_windows_file_whose_data_is_local_is_not_a_placeholder() {
    for attributes in [
        0,
        ARCHIVE | READONLY,
        // Hydrated in a synced folder: still the provider's reparse point, its data all here.
        REPARSE_POINT | ARCHIVE,
        REPARSE_POINT | PINNED,
        UNPINNED,
    ] {
        assert!(!placeholder_by_attributes(attributes), "{attributes:#x}");
    }
}

#[test]
fn a_macos_file_is_a_placeholder_exactly_when_it_is_dataless() {
    assert!(placeholder_by_flags(SF_DATALESS));
    assert!(placeholder_by_flags(SF_DATALESS | UF_HIDDEN));
    assert!(!placeholder_by_flags(0));
    assert!(!placeholder_by_flags(UF_HIDDEN | SF_IMMUTABLE));
}

#[test]
fn a_file_whose_data_is_on_the_disk_is_no_placeholder_on_this_platform() {
    // This crate's own manifest, which the build just read, so its data is local wherever it runs.
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let metadata = std::fs::metadata(manifest).expect("the manifest");

    assert!(!is_cloud_placeholder(&metadata));
}
