//! `Status` (what a `Link` call returns) and `Error` (what the codecs return).

use crate::wire::{Reader, Wire};
use alloc::vec::Vec;
use core::fmt;

/// Result code of a `Link` call. 0 = success, negative = failure.
///
/// The bridge maps these to whatever its transport uses (`NTSTATUS` for the
/// Wine unixlib); proto does not know that transport.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Status(pub i32);

impl Status {
    pub const OK: Status = Status(0);
    /// A pointer, length or field value is not acceptable.
    pub const INVALID_ARG: Status = Status(-1);
    /// The block or stream carries a version this side does not know.
    pub const BAD_VERSION: Status = Status(-2);
    /// `SyncOp` number this side does not know.
    pub const UNKNOWN_OP: Status = Status(-3);
    /// Known to proto but not implemented by this back yet (reserved or placeholder).
    pub const UNSUPPORTED: Status = Status(-4);
    /// The command stream is malformed (an `Error` from the decoder).
    pub const BAD_STREAM: Status = Status(-5);
    /// A handle does not name a live object.
    pub const NOT_FOUND: Status = Status(-6);
    pub const OUT_OF_MEMORY: Status = Status(-7);
    /// The device is gone (maps to "device removed").
    pub const DEVICE_LOST: Status = Status(-8);
    /// The GPU took too long (maps to "device hung").
    pub const DEVICE_HUNG: Status = Status(-9);
    /// A wait ran out of time.
    pub const TIMEOUT: Status = Status(-10);
    /// `Hello` has not been called, or the device was shut down.
    pub const NOT_INITIALIZED: Status = Status(-11);
    pub const INTERNAL: Status = Status(-12);
    /// Rule 6: a physical page already reaches the GPU through another buffer.
    pub const CONFLICT: Status = Status(-13);

    pub const fn is_ok(self) -> bool {
        self.0 == 0
    }

    pub const fn is_err(self) -> bool {
        self.0 != 0
    }

    /// `Ok(())` for success, `Err(self)` otherwise.
    pub const fn ok(self) -> Result<(), Status> {
        if self.0 == 0 { Ok(()) } else { Err(self) }
    }

    pub const fn name(self) -> &'static str {
        match self.0 {
            0 => "OK",
            -1 => "INVALID_ARG",
            -2 => "BAD_VERSION",
            -3 => "UNKNOWN_OP",
            -4 => "UNSUPPORTED",
            -5 => "BAD_STREAM",
            -6 => "NOT_FOUND",
            -7 => "OUT_OF_MEMORY",
            -8 => "DEVICE_LOST",
            -9 => "DEVICE_HUNG",
            -10 => "TIMEOUT",
            -11 => "NOT_INITIALIZED",
            -12 => "INTERNAL",
            -13 => "CONFLICT",
            _ => "?",
        }
    }
}

impl Wire for Status {
    const SIZE: usize = 4;
    const ZERO: Self = Status(0);
    fn put(&self, out: &mut Vec<u8>) {
        self.0.put(out);
    }
    fn get(r: &mut Reader<'_>) -> Result<Self, Error> {
        Ok(Status(i32::get(r)?))
    }
}

impl fmt::Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Status::{}({})", self.name(), self.0)
    }
}

/// Why a stream, a file or a description was refused. Never a panic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes end before the thing they describe does.
    Truncated,
    /// The stream or file does not start with the proto magic.
    BadMagic,
    /// Stream or file written for another proto version.
    BadProtoVersion { found: u32 },
    /// A header field (length, reserved flags) is not what this version writes.
    BadHeader,
    /// `total_len` in the stream header differs from the block size.
    BadTotalLength { declared: u32, actual: usize },
    /// The stream header counts more or fewer commands than the block holds.
    CommandCountMismatch { declared: u32, found: u32 },
    /// Bytes after the last declared command.
    TrailingBytes,
    /// The opcode is not defined by this version of proto.
    UnknownOpcode(u16),
    /// The opcode is inside a range proto reserved for a later milestone.
    ReservedOpcode(u16),
    /// The opcode exists but not in this version.
    UnknownOpcodeVersion { opcode: u16, version: u8 },
    /// Payload length out of bounds, not a multiple of 8, or not what the opcode needs.
    BadPayloadLength { opcode: u16, len: u32 },
    /// A field holds a value the opcode does not allow (unknown enum, stray bits).
    BadValue { opcode: u16, field: &'static str },
    /// Encoder: a block, a label or a constant run is larger than the format allows.
    TooLarge,
}

impl Error {
    /// The `Status` a back answers with.
    pub const fn status(self) -> Status {
        match self {
            Error::BadProtoVersion { .. } | Error::UnknownOpcodeVersion { .. } => {
                Status::BAD_VERSION
            }
            Error::ReservedOpcode(_) => Status::UNSUPPORTED,
            Error::TooLarge => Status::INVALID_ARG,
            _ => Status::BAD_STREAM,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => write!(f, "truncated"),
            Error::BadMagic => write!(f, "bad magic"),
            Error::BadProtoVersion { found } => write!(f, "unsupported proto version {found}"),
            Error::BadHeader => write!(f, "bad header"),
            Error::BadTotalLength { declared, actual } => {
                write!(f, "header says {declared} bytes, block has {actual}")
            }
            Error::CommandCountMismatch { declared, found } => {
                write!(f, "header says {declared} commands, found {found}")
            }
            Error::TrailingBytes => write!(f, "bytes after the last command"),
            Error::UnknownOpcode(op) => write!(f, "unknown opcode {op:#06x}"),
            Error::ReservedOpcode(op) => write!(f, "opcode {op:#06x} is reserved, not defined yet"),
            Error::UnknownOpcodeVersion { opcode, version } => {
                write!(f, "opcode {opcode:#06x} has no version {version}")
            }
            Error::BadPayloadLength { opcode, len } => {
                write!(f, "opcode {opcode:#06x}: bad payload length {len}")
            }
            Error::BadValue { opcode, field } => {
                write!(f, "opcode {opcode:#06x}: bad value in `{field}`")
            }
            Error::TooLarge => write!(f, "too large for the format"),
        }
    }
}
