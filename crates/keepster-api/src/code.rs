//! The error codes a problem document may carry: one table, from which the enum, its wire spelling,
//! its meaning and the `/v1` document's list are all generated.

/// Declare every code once, so that adding one is a single line and nothing can list a subset.
macro_rules! error_codes {
    ($( $variant:ident = $code:literal: $meaning:literal, )*) => {
        /// The `code` of a problem document: the stable, machine-readable part of a failure.
        ///
        /// The set is **open** (ADR 0005): it grows inside `/v1`, so it travels as a string and
        /// [`Problem::code`](crate::Problem::code) stays a `String`. A client parses it with
        /// [`ErrorCode::parse`] and must treat a code it does not know as the problem's HTTP status.
        /// Deliberately not `#[non_exhaustive]`: the CLI maps every variant to an exit code with an
        /// exhaustive `match`, so a new code cannot silently acquire a default.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum ErrorCode {
            $(
                #[doc = $meaning]
                $variant,
            )*
        }

        impl ErrorCode {
            /// Every code this build knows, in declaration order.
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            /// How the code is spelled on the wire.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)*
                }
            }

            /// What the code means, in one sentence. Documentation, not contract.
            pub const fn meaning(self) -> &'static str {
                match self {
                    $(Self::$variant => $meaning,)*
                }
            }
        }
    };
}

error_codes! {
    RootNotFound = "root_not_found": "No Root with this identity is registered.",
    SnapshotNotFound = "snapshot_not_found": "No Snapshot with this identity is in the Root's history.",
    PathNotFound = "path_not_found": "The Snapshot holds nothing at this path.",
    RootOverlaps = "root_overlaps": "The directory is inside, or contains, a Root that is already registered.",
    RootUnavailable = "root_unavailable": "The Root cannot be served: its directory or its record is missing or unreadable.",
    NotAFile = "not_a_file": "The path names something other than a file, so it has no content to read.",
    NotADirectory = "not_a_directory": "The path names something other than a directory, so it has no entries to list.",
    PathInvalid = "path_invalid": "A path is malformed: not absolute where it must be, or not Root-relative where it must be.",
    LimitInvalid = "limit_invalid": "`limit` is not a positive whole number.",
    CursorInvalid = "cursor_invalid": "`cursor` is not a `next_cursor` this API returned.",
    MomentInvalid = "moment_invalid": "A moment is not an RFC 3339 timestamp.",
    DepthInvalid = "depth_invalid": "`depth` is neither `1` nor `infinity`.",
    RegistryFailed = "registry_failed": "The Registry of Watched Roots could not be read or written.",
    CaptureFailed = "capture_failed": "A Snapshot could not be written.",
    ClockUnusable = "clock_unusable": "The system clock is outside the range a Snapshot can record.",
    HistoryUnreadable = "history_unreadable": "The Root's history could not be read.",
    BodyInvalid = "body_invalid": "The request body is not the JSON this endpoint takes.",
    PurgeInvalid = "purge_invalid": "`purge` is neither `true` nor `false`.",
    ConfigInvalid = "config_invalid": "The Root's configuration cannot be read, so nothing is captured on terms it does not state.",
    ComparisonInvalid = "comparison_invalid": "`from` is not a Snapshot's identity, or `to` is neither one nor `working`.",
    FormatInvalid = "format_invalid": "`format` is neither `json` nor `unified`.",
    ConfirmationRequired = "confirmation_required": "A path of the request needs a confirmation or a resolution, or cannot be restored: nothing was written, and `paths` says what each needs.",
    ResolutionInvalid = "resolution_invalid": "A resolution sets no answer or more than one, or names a new name that is not a bare name.",
    Withheld = "withheld": "The Snapshot holds no content at this path on purpose, so there is none to read or restore; `details.reason` says why.",
    JobNotFound = "job_not_found": "No job with this identity is known: none was started, or the daemon has restarted since, which forgets every job.",
    MaintenanceUnavailable = "maintenance_unavailable": "No `git` a Maintenance Pass can run with is usable; `GET /v1/status` says why and how to install one.",
    MaintenanceFailed = "maintenance_failed": "A Maintenance Pass ran and failed; `detail` holds what `git` said.",
    JobInterrupted = "job_interrupted": "The daemon stopped before the job could start; nothing was done.",
    GlobInvalid = "glob_invalid": "`glob` is not a pattern Forget can remove: malformed, or a negation, which names what to keep.",
    SecretOptedIn = "secret_opted_in": "A path the glob matches is on the secret opt-in list, which says it is to be captured, so it cannot be excluded from now on; `details.path` names it.",
    VolumeMismatch = "volume_mismatch": "The Root's directory is not on the volume it was registered on, so nothing is captured from or restored into it; its history is as readable as ever.",
    StoreUnreachable = "store_unreachable": "The Root's History Store cannot be opened or written, so nothing is captured: there is no other store to write to instead.",
}

impl ErrorCode {
    /// The code a wire string names, or `None` for one this build does not know — which a client must
    /// then handle as the problem's HTTP status.
    pub fn parse(code: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|known| known.as_str() == code)
    }
}

/// `code` as the `/v1` document describes it: a string, with the known values in its description.
///
/// Not a schema `enum`, deliberately (ADR 0005): a closed enum in a response promises that no other code
/// will ever appear, which this API does not promise, and the compatibility check would flag every added
/// code as breaking. The list is generated from [`ErrorCode::ALL`], so it cannot disagree with the enum.
#[cfg(feature = "openapi")]
pub(crate) fn code_schema() -> utoipa::openapi::Object {
    use utoipa::openapi::{ObjectBuilder, Type};

    let mut description = String::from(
        "The stable, machine-readable code of the problem. Contract, unlike the prose of `title` and \
         `detail`.\n\nThe set is open: it may grow inside `/v1`, so a client must treat an unknown code \
         as the problem's HTTP status. The codes this version knows:\n",
    );
    for code in ErrorCode::ALL {
        description.push_str(&format!("\n- `{}`: {}", code.as_str(), code.meaning()));
    }

    ObjectBuilder::new()
        .schema_type(Type::String)
        .description(Some(description))
        .build()
}

#[cfg(test)]
mod tests {
    use super::ErrorCode;

    #[test]
    fn every_known_code_is_parsed_from_its_wire_spelling() {
        for &code in ErrorCode::ALL {
            assert_eq!(ErrorCode::parse(code.as_str()), Some(code));
        }
    }

    #[test]
    fn a_code_this_build_does_not_know_parses_to_nothing() {
        assert_eq!(ErrorCode::parse("from_the_future"), None);
        assert_eq!(ErrorCode::parse(""), None);
    }
}
