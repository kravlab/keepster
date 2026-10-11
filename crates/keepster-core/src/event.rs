//! The identity of one event on the daemon's live stream (ticket 45).

use crate::identity::ulid_identity;

ulid_identity! {
    /// One event's identity on the live stream: what a reconnecting client sends back as
    /// `Last-Event-ID` to have the daemon replay what followed it.
    ///
    /// A ULID like every other identity here, so it is unique without coordination and says roughly
    /// when the event happened; the replay itself goes by the order the daemon kept, not by comparing
    /// identities, since two events minted in the same millisecond carry no order between them.
    EventId
}
