//! Opaque identifiers.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! numeric_id {
    ($(#[$meta:meta])* $name:ident, $prefix:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            /// The next identifier in sequence.
            #[must_use]
            pub const fn next(self) -> Self {
                Self(self.0 + 1)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }
    };
}

numeric_id!(
    /// Identifies one inquiry (one question being worked on).
    InquiryId,
    "inquiry#"
);
numeric_id!(
    /// Identifies one episode (one bounded run of the system on an inquiry).
    EpisodeId,
    "episode#"
);
numeric_id!(
    /// Identifies one external observation. Evidence that traces back to the
    /// same observation is not independent, however it was redescribed.
    ObservationId,
    "obs#"
);
numeric_id!(
    /// Identifies one resource receipt.
    ReceiptId,
    "receipt#"
);

macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Creates an identifier from any string-like value.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The identifier as a string slice.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(
    /// Names a registered operator.
    OperatorId
);
string_id!(
    /// Names a view inside an inquiry.
    ViewId
);
string_id!(
    /// Names a frame (a representation system).
    FrameId
);
