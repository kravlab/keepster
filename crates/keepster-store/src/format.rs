//! Which content format a Segment or a Pin was written in (ADR 0010, ticket 55).
//!
//! There is no migration: the format is a property of each Segment, fixed at its parentless root commit,
//! and a store may hold a mix indefinitely. Every commit this build writes carries it, and no build ever
//! appends to a Segment of another format, so a commit that names its format speaks for its whole
//! Segment — which keeps the question one commit deep for every Segment written since. Only one that
//! does not say, written before the format was recorded, is walked to its root.

use keepster_core::ContentFormat;

use crate::history::FORMAT_TRAILER;

/// The format of the Segment or Pin whose tip is `tip`, or `None` when what records it names none.
pub(crate) fn of(
    repository: &git2::Repository,
    tip: git2::Oid,
) -> Result<Option<ContentFormat>, git2::Error> {
    let mut commit = repository.find_commit(tip)?;
    loop {
        if let Some(named) = named(&commit) {
            return Ok(named);
        }
        if commit.parent_count() == 0 {
            // A root that does not say was written before the format was recorded.
            return Ok(Some(ContentFormat::FIRST));
        }
        commit = commit.parent(0)?;
    }
}

/// What `commit` says of its format: nothing, or the format it names — `None` within when that is no
/// format at all, or the message cannot be read to tell.
fn named(commit: &git2::Commit<'_>) -> Option<Option<ContentFormat>> {
    let Ok(message) = commit.message() else {
        return Some(None);
    };
    let trailers = git2::message_trailers_strs(message).ok()?;
    let value = trailers
        .iter()
        .find(|(key, _)| *key == FORMAT_TRAILER)
        .map(|(_, value)| value)?;

    Some(ContentFormat::parse(value))
}
