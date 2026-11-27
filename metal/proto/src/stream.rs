//! A block of commands: stream header, encoder, decoder.
//!
//! ```text
//! block  = StreamHeader (32 bytes) command*
//! command = opcode u16 | version u8 | flags u8 (0) | len u32 | payload[len]   len % 8 == 0
//! ```
//! A block is what `Link::submit` takes (one `ExecuteCommandLists`), and it is
//! also the unit of a dump file (see [`crate::file`]).

use crate::PROTO_VERSION;
use crate::cmd::{CMD_HEADER_SIZE, Command, decode_one};
use crate::status::Error;
use crate::types::Caps;
use crate::wire::{Reader, Wire, wire_struct};
use alloc::vec::Vec;

/// First eight bytes of every block.
pub const STREAM_MAGIC: [u8; 8] = *b"VMPSTRM\0";

wire_struct! {
    pub struct StreamHeader {
        magic: [u8; 8],
        proto_version: u32,
        /// Bytes of this header (32); commands start there.
        header_len: u16,
        /// Reserved, 0.
        flags: u16,
        /// Capabilities the stream needs; a replayer may refuse a back that lacks them.
        caps: Caps,
        /// Bytes of the whole block, header included; a multiple of 8.
        total_len: u32,
        /// Number of commands.
        count: u32,
    }
}

/// Bytes of a [`StreamHeader`].
pub const STREAM_HEADER_SIZE: usize = 32;

impl StreamHeader {
    /// Parses and checks the header of `block` against the block's length.
    pub fn parse(block: &[u8]) -> Result<Self, Error> {
        let h = StreamHeader::get(&mut Reader::new(block))?;
        if h.magic != STREAM_MAGIC {
            return Err(Error::BadMagic);
        }
        if h.proto_version != PROTO_VERSION {
            return Err(Error::BadProtoVersion {
                found: h.proto_version,
            });
        }
        if h.header_len as usize != STREAM_HEADER_SIZE || h.flags != 0 {
            return Err(Error::BadHeader);
        }
        if h.total_len as usize != block.len() {
            return Err(Error::BadTotalLength {
                declared: h.total_len,
                actual: block.len(),
            });
        }
        if !h.total_len.is_multiple_of(8) {
            return Err(Error::BadHeader);
        }
        // Every command is at least a header, so the count bounds itself.
        if (h.count as usize) > (block.len() - STREAM_HEADER_SIZE) / CMD_HEADER_SIZE {
            return Err(Error::CommandCountMismatch {
                declared: h.count,
                found: 0,
            });
        }
        Ok(h)
    }
}

/// Builds a block.
#[derive(Clone, Debug)]
pub struct Encoder {
    buf: Vec<u8>,
    count: u32,
}

impl Encoder {
    /// A block that needs `caps` (use `Caps::NONE` when unsure).
    pub fn new(caps: Caps) -> Self {
        let mut buf = Vec::with_capacity(256);
        StreamHeader {
            magic: STREAM_MAGIC,
            proto_version: PROTO_VERSION,
            header_len: STREAM_HEADER_SIZE as u16,
            flags: 0,
            caps,
            total_len: 0,
            count: 0,
        }
        .put(&mut buf);
        Encoder { buf, count: 0 }
    }

    /// Appends a command. On error the block is left as it was.
    pub fn push(&mut self, cmd: &Command<'_>) -> Result<(), Error> {
        let before = self.buf.len();
        if let Err(e) = cmd.encode(&mut self.buf) {
            self.buf.truncate(before);
            return Err(e);
        }
        if self.buf.len() > u32::MAX as usize {
            self.buf.truncate(before);
            return Err(Error::TooLarge);
        }
        self.count += 1;
        Ok(())
    }

    /// `SET_ROOT_CONSTANTS` from native values.
    pub fn set_root_constants(
        &mut self,
        bind_point: crate::types::BindPoint,
        root_index: u32,
        dest_offset: u32,
        values: &[u32],
    ) -> Result<(), Error> {
        if values.len() > crate::cmd::MAX_ROOT_CONSTANTS {
            return Err(Error::TooLarge);
        }
        let mut bytes = [0u8; crate::cmd::MAX_ROOT_CONSTANTS * 4];
        for (chunk, v) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            chunk.copy_from_slice(&v.to_le_bytes());
        }
        self.push(&Command::SetRootConstants(crate::cmd::SetRootConstants {
            bind_point,
            root_index,
            dest_offset,
            data: &bytes[..values.len() * 4],
        }))
    }

    /// Commands so far.
    pub fn count(&self) -> u32 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Bytes so far, header included.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Finishes the block (fills in `total_len` and `count`).
    pub fn finish(mut self) -> Vec<u8> {
        let total = self.buf.len() as u32;
        let at = core::mem::offset_of!(StreamHeader, total_len);
        self.buf[at..at + 4].copy_from_slice(&total.to_le_bytes());
        let at = core::mem::offset_of!(StreamHeader, count);
        self.buf[at..at + 4].copy_from_slice(&self.count.to_le_bytes());
        self.buf
    }
}

/// Iterates the commands of a block without copying. After the first error it
/// yields nothing more. At the end it checks that the header's count and
/// length were honest.
#[derive(Clone, Debug)]
pub struct Decoder<'a> {
    header: StreamHeader,
    rest: &'a [u8],
    left: u32,
    found: u32,
    done: bool,
}

impl<'a> Decoder<'a> {
    /// Checks the header (magic, version, lengths); commands are checked as
    /// they are read.
    pub fn new(block: &'a [u8]) -> Result<Self, Error> {
        let header = StreamHeader::parse(block)?;
        Ok(Decoder {
            header,
            rest: &block[STREAM_HEADER_SIZE..],
            left: header.count,
            found: 0,
            done: false,
        })
    }

    pub fn header(&self) -> &StreamHeader {
        &self.header
    }

    /// Reads the whole block and returns every command, or the first error.
    pub fn collect_all(self) -> Result<Vec<Command<'a>>, Error> {
        self.collect()
    }
}

impl<'a> Iterator for Decoder<'a> {
    type Item = Result<Command<'a>, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if self.left == 0 {
            self.done = true;
            return if self.rest.is_empty() {
                None
            } else {
                Some(Err(Error::TrailingBytes))
            };
        }
        if self.rest.is_empty() {
            self.done = true;
            return Some(Err(Error::CommandCountMismatch {
                declared: self.header.count,
                found: self.found,
            }));
        }
        match decode_one(self.rest) {
            Ok((cmd, used)) => {
                self.rest = &self.rest[used..];
                self.left -= 1;
                self.found += 1;
                Some(Ok(cmd))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}
