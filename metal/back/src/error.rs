//! The error type of the executor core.

use std::fmt;

/// What went wrong. No variant carries a Metal object, so the proto
/// interpreter can turn each into a status code without touching Metal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `MTLCreateSystemDefaultDevice` returned nil.
    NoDevice,
    /// A hard requirement is not met (Metal 4 family, Argument Buffers
    /// Tier 2, ...). The backend does not fall back to anything.
    Unsupported(String),
    /// Metal returned nil or an `NSError`.
    Metal(String),
    /// `wrap` would put a physical page behind a second GPU buffer
    /// (layering rule 6).
    PageConflict {
        /// First page-rounded byte of the rejected range.
        start: usize,
        /// One past the last page-rounded byte of the rejected range.
        end: usize,
        /// The live range it collides with, page-rounded as well.
        existing: (usize, usize),
    },
    /// A caller-supplied value is out of range or misaligned.
    BadArgument(String),
    /// A GPU address or resource is not (or no longer) resident.
    NotResident(String),
    /// A wait ran out of time.
    Timeout,
    /// The queue reported a failure that makes further work meaningless.
    DeviceLost(crate::queue::DeviceStatus),
}

/// `Result` with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoDevice => write!(f, "no Metal device"),
            Error::Unsupported(what) => write!(f, "unsupported: {what}"),
            Error::Metal(what) => write!(f, "Metal: {what}"),
            Error::PageConflict {
                start,
                end,
                existing,
            } => write!(
                f,
                "pages {start:#x}..{end:#x} are already behind another GPU buffer ({:#x}..{:#x})",
                existing.0, existing.1
            ),
            Error::BadArgument(what) => write!(f, "bad argument: {what}"),
            Error::NotResident(what) => write!(f, "not resident: {what}"),
            Error::Timeout => write!(f, "timed out"),
            Error::DeviceLost(status) => write!(f, "device lost: {status:?}"),
        }
    }
}

impl std::error::Error for Error {}
