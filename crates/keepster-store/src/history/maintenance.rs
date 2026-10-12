//! The Maintenance Pass (ticket 52): what Retention made unreachable is pruned at once, and what is left
//! is packed.

use std::path::Path;
use std::time::SystemTime;

use super::HistoryStore;
use crate::git::{Git, GitUnavailable};

/// The file whose modification time is when the last Maintenance Pass over this store finished, so a
/// restarted daemon does not maintain every store again at once.
const MAINTAINED: &str = "keepster-maintained";

/// What a Maintenance Pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Maintained {
    /// How much smaller the store's objects are on disk than before the pass.
    pub reclaimed_bytes: u64,
}

impl HistoryStore {
    /// Reclaim the space nothing references any more, and pack what is left.
    ///
    /// `gc --prune=now` rather than git's own fortnight's grace, which would leave everything Retention
    /// just unlinked on disk for two weeks — and only safe because the caller holds the store's lock, so
    /// no capture writes meanwhile (ticket 06). Never `--force`, which defeats the lock `gc` keeps against
    /// another `gc`, and never `git maintenance`, whose defaults keep moving; `gc` packs the refs too.
    ///
    /// The error is `git`'s own, word for word: a pass fails only through what `git` did or said.
    pub fn maintain(&self, git: &Git) -> Result<Maintained, GitUnavailable> {
        let objects = self.path.join("objects");
        let before = bytes_under(&objects);
        git.run_in(&self.path, &["gc", "--prune=now", "--quiet"])?;
        // Best effort: without it, the next daemon maintains this store once more than it needed to.
        let _ = std::fs::write(self.path.join(MAINTAINED), b"");

        Ok(Maintained {
            reclaimed_bytes: before.saturating_sub(bytes_under(&objects)),
        })
    }

    /// When the last Maintenance Pass over the store at `path` finished, as far as the store records it —
    /// read without opening the store or its index.
    pub fn maintained_at(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path.join(MAINTAINED))
            .and_then(|metadata| metadata.modified())
            .ok()
    }
}

/// Every byte of every file under `directory`, as far as it can be read.
fn bytes_under(directory: &Path) -> u64 {
    std::fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| match entry.file_type() {
                    Ok(kind) if kind.is_dir() => bytes_under(&entry.path()),
                    _ => entry.metadata().map_or(0, |metadata| metadata.len()),
                })
                .sum()
        })
        .unwrap_or(0)
}
