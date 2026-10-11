//! The Maintenance Pass (ticket 52): what Retention made unreachable is pruned at once, and what is left
//! is packed.

use std::path::Path;
use std::time::SystemTime;

use super::HistoryStore;
use crate::git::{Git, GitUnavailable};

/// The file whose modification time is when the last Maintenance Pass over this store finished, so a
/// restarted daemon does not maintain every store again at once.
const MAINTAINED: &str = "keepster-maintained";

impl HistoryStore {
    /// Reclaim the space nothing references any more, and pack what is left.
    ///
    /// `gc --prune=now` rather than git's own fortnight's grace, which would leave everything Retention
    /// just unlinked on disk for two weeks — and only safe because the caller holds the store's lock, so
    /// no capture writes meanwhile (ticket 06). Never `--force`, which defeats the lock `gc` keeps against
    /// another `gc`, and never `git maintenance`, whose defaults keep moving; `gc` packs the refs too.
    ///
    /// The error is `git`'s own, word for word: a pass fails only through what `git` did or said.
    pub fn maintain(&self, git: &Git) -> Result<(), GitUnavailable> {
        git.run_in(&self.path, &["gc", "--prune=now", "--quiet"])?;
        // Best effort: without it, the next daemon maintains this store once more than it needed to.
        let _ = std::fs::write(self.path.join(MAINTAINED), b"");

        Ok(())
    }

    /// When the last Maintenance Pass over the store at `path` finished, as far as the store records it —
    /// read without opening the store or its index.
    pub fn maintained_at(path: &Path) -> Option<SystemTime> {
        std::fs::metadata(path.join(MAINTAINED))
            .and_then(|metadata| metadata.modified())
            .ok()
    }
}
