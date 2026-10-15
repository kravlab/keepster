//! Which files a capture meets are cloud placeholders, so that none is ever opened (ticket 75).

use std::fmt;
use std::fs;
use std::path::Path;

/// What a capture asks about a file before it opens it: whether its data is only in the cloud. The
/// platform itself in production; a stand-in in tests, since no test machine has a sync client — as
/// the daemon's `Volumes` is for which disk a path is on.
pub trait CloudFiles: fmt::Debug + Send + Sync {
    /// Whether the file at `path`, which `metadata` describes, is a placeholder: opening or reading it
    /// would download it.
    fn is_placeholder(&self, path: &Path, metadata: &fs::Metadata) -> bool;
}

/// The platform's own answer, read from the metadata a `stat` or a directory listing already returned:
/// the file attributes on Windows, `st_flags` on macOS. Linux has no placeholders, so none is found
/// there (ticket 75).
#[derive(Debug, Clone, Copy, Default)]
pub struct PlatformCloudFiles;

impl CloudFiles for PlatformCloudFiles {
    fn is_placeholder(&self, _path: &Path, metadata: &fs::Metadata) -> bool {
        keepster_core::is_cloud_placeholder(metadata)
    }
}
