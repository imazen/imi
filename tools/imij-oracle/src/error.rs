//! The specification's error classes (SPEC section 0.1).

use core::fmt;

/// A decode failure.
///
/// SPEC distinguishes two classes: `Unsupported` for syntax this decoder does
/// not implement (an unknown feature, a retired entropy value) and `Corrupt`
/// for a stream that violates the grammar. Resource limits are reported as
/// `Corrupt` where the stream cannot be represented, per this decoder's
/// scalar design.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A declared feature or syntax element this decoder does not implement.
    Unsupported(String),
    /// A malformed or inconsistent stream.
    Corrupt(String),
}

impl Error {
    pub(crate) fn unsupported(what: impl Into<String>) -> Self {
        Error::Unsupported(what.into())
    }

    pub(crate) fn corrupt(what: impl Into<String>) -> Self {
        Error::Corrupt(what.into())
    }

    /// The class name as SPEC writes it, for reports.
    pub fn class(&self) -> &'static str {
        match self {
            Error::Unsupported(_) => "Unsupported",
            Error::Corrupt(_) => "Corrupt",
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::Corrupt(what) => write!(f, "corrupt: {what}"),
        }
    }
}

impl core::error::Error for Error {}

pub(crate) type Result<T> = core::result::Result<T, Error>;
