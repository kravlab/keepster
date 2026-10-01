//! Identities: ULIDs minted from a moment.
//!
//! Several things in this product need an identity that outlives what it names — a Root outlives its
//! path, a Snapshot outlives the commit that holds it — and they are identical in every respect but
//! their name and their documentation. A macro keeps them one implementation; a generic type with a
//! phantom tag would do the same but would put `UlidIdentity<RootTag>` into every message a person
//! reads.

/// Why a string is not an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not an identity")]
pub struct NotAnIdentity;

/// Define one identity type: a ULID behind a name of its own, so two kinds of identity cannot be
/// mistaken for each other even though they are the same bits.
macro_rules! ulid_identity {
    ($(#[$attribute:meta])* $name:ident) => {
        $(#[$attribute])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(::ulid::Ulid);

        impl $name {
            /// Mint an identity for something that came into being at `now`.
            ///
            /// The moment is a parameter rather than a clock read: a ULID embeds its timestamp, and
            /// ADR 0012 keeps every wall-clock read in `keepster-daemon`. It is also what makes a
            /// month of history reproducible in a test.
            pub fn generate(now: ::std::time::SystemTime) -> Self {
                Self(::ulid::Ulid::from_datetime(now))
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                write!(formatter, "{}", self.0)
            }
        }

        impl ::std::str::FromStr for $name {
            type Err = $crate::NotAnIdentity;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                ::ulid::Ulid::from_string(text)
                    .map(Self)
                    .map_err(|_| $crate::NotAnIdentity)
            }
        }
    };
}

pub(crate) use ulid_identity;
