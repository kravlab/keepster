//! Whether a file is a cloud placeholder — there by name and size, its data only in the cloud — read
//! from what a walk's `stat` already holds, so it is told without opening the file (ticket 75).
//!
//! Opening or reading a placeholder is what downloads it: the first pass over a synced folder would pull
//! the whole cloud down, fill the disk and undo the user's "free up space". Both platforms say it in the
//! metadata a directory listing returns. Linux has no such notion: a FUSE client fetches on read and says
//! nothing beforehand, so nothing there is recognised and every file is read as any other.
//!
//! The bit tests are plain arithmetic and run everywhere; `is_cloud_placeholder` applies the platform's
//! own to a `stat` already taken, so the capture and the watcher ask the same question the same way.

use std::fs;

/// `FILE_ATTRIBUTE_OFFLINE`: the data was moved to offline storage.
const OFFLINE: u32 = 0x1000;
/// `FILE_ATTRIBUTE_RECALL_ON_OPEN`: the item is virtual, and opening it fetches it.
const RECALL_ON_OPEN: u32 = 0x4_0000;
/// `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`: not all of the data is local, and reading it fetches the rest.
const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
/// `SF_DATALESS`: the file is a dataless object, its content materialised on access.
const SF_DATALESS: u32 = 0x4000_0000;

/// Whether a file with the Windows file `attributes` is a placeholder: any of its data elsewhere, so that
/// opening or reading it would fetch it.
///
/// A hydrated file in a synced folder keeps its provider's reparse point and may be pinned or unpinned,
/// none of which makes it a placeholder: its data is all here.
pub fn placeholder_by_attributes(attributes: u32) -> bool {
    attributes & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

/// Whether a file with the macOS `st_flags` is a placeholder: dataless, its content in a File Provider's
/// cloud.
pub fn placeholder_by_flags(flags: u32) -> bool {
    flags & SF_DATALESS != 0
}

/// Whether the file `metadata` describes is a cloud placeholder: by its file attributes on Windows, by
/// its `st_flags` on macOS. Linux has no placeholders, so nothing is one there.
///
/// It reads only what the `stat` that produced `metadata` already returned, and opens nothing.
pub fn is_cloud_placeholder(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        placeholder_by_attributes(metadata.file_attributes())
    }

    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;

        placeholder_by_flags(metadata.st_flags())
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = metadata;
        false
    }
}
