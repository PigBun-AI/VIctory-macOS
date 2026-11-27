//! Dump/replay files: the same blocks, with an optional file header in front.
//!
//! ```text
//! file = FileHeader (64 bytes) block*        (blocks as in `stream`, each a multiple of 8 bytes)
//! ```
//! A file is for blocks and sync-call records captured from a front (or made
//! by a test) and replayed against a back with no Wine and no game. A bare
//! block (what `Link::submit` takes) is also a valid, header-less dump: the
//! first eight bytes tell the two apart (`FILE_MAGIC` vs `STREAM_MAGIC`).
//! Sync calls are recorded as `SYNC_RECORD` commands, CPU writes into mapped
//! buffers as `BUFFER_DATA`, in blocks of their own or among the commands.
//! Dumps made from a game run are derived from game data and never go into
//! the repository; synthetic ones may.

use crate::PROTO_VERSION;
use crate::status::Error;
use crate::stream::{STREAM_HEADER_SIZE, StreamHeader};
use crate::types::Caps;
use crate::wire::{Reader, Wire, wire_struct};
use alloc::vec::Vec;

pub const FILE_MAGIC: [u8; 8] = *b"VMPFILE\0";
/// Version of the container (this header and block framing). Independent of
/// `PROTO_VERSION`, which versions what is inside the blocks; a file needs both to match.
pub const FILE_VERSION: u32 = 1;
pub const FILE_HEADER_SIZE: usize = 64;

wire_struct! {
    pub struct FileHeader {
        magic: [u8; 8],
        file_version: u32,
        proto_version: u32,
        header_len: u16,
        flags: u16,
        /// Number of blocks; 0 = not recorded, read until the end.
        block_count: u32,
        /// Union of the blocks' capability needs.
        caps: Caps,
        /// Who wrote it, NUL-padded text ("front 0.1").
        creator: [u8; 16],
        reserved: [u32; 4],
    }
}

impl FileHeader {
    pub fn parse(file: &[u8]) -> Result<Self, Error> {
        let h = FileHeader::get(&mut Reader::new(file))?;
        if h.magic != FILE_MAGIC {
            return Err(Error::BadMagic);
        }
        if h.file_version != FILE_VERSION {
            return Err(Error::BadProtoVersion {
                found: h.file_version,
            });
        }
        if h.proto_version != PROTO_VERSION {
            return Err(Error::BadProtoVersion {
                found: h.proto_version,
            });
        }
        if h.header_len as usize != FILE_HEADER_SIZE || h.flags != 0 {
            return Err(Error::BadHeader);
        }
        Ok(h)
    }
}

/// Builds a dump file in memory.
#[derive(Clone, Debug)]
pub struct FileWriter {
    buf: Vec<u8>,
    blocks: u32,
    caps: Caps,
}

impl FileWriter {
    /// `creator` is truncated to 16 bytes.
    pub fn new(creator: &str) -> Self {
        let mut c = [0u8; 16];
        let n = creator.len().min(16);
        c[..n].copy_from_slice(&creator.as_bytes()[..n]);
        let mut buf = Vec::new();
        FileHeader {
            magic: FILE_MAGIC,
            file_version: FILE_VERSION,
            proto_version: PROTO_VERSION,
            header_len: FILE_HEADER_SIZE as u16,
            flags: 0,
            block_count: 0,
            caps: Caps::NONE,
            creator: c,
            reserved: [0; 4],
        }
        .put(&mut buf);
        FileWriter {
            buf,
            blocks: 0,
            caps: Caps::NONE,
        }
    }

    /// Appends a block after checking its header.
    pub fn push_block(&mut self, block: &[u8]) -> Result<(), Error> {
        let h = StreamHeader::parse(block)?;
        self.caps = self.caps | h.caps;
        self.buf.extend_from_slice(block);
        self.blocks += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Vec<u8> {
        let at = core::mem::offset_of!(FileHeader, block_count);
        self.buf[at..at + 4].copy_from_slice(&self.blocks.to_le_bytes());
        let at = core::mem::offset_of!(FileHeader, caps);
        self.buf[at..at + 8].copy_from_slice(&self.caps.0.to_le_bytes());
        self.buf
    }
}

/// Iterates the blocks of a dump file. Each item is a slice ready for
/// `Decoder::new` or `Link::submit`.
#[derive(Clone, Debug)]
pub struct FileReader<'a> {
    header: FileHeader,
    rest: &'a [u8],
    found: u32,
    done: bool,
}

impl<'a> FileReader<'a> {
    pub fn new(file: &'a [u8]) -> Result<Self, Error> {
        let header = FileHeader::parse(file)?;
        Ok(FileReader {
            header,
            rest: &file[FILE_HEADER_SIZE..],
            found: 0,
            done: false,
        })
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    /// `true` when `bytes` starts like a dump file rather than a bare block.
    pub fn looks_like_file(bytes: &[u8]) -> bool {
        bytes.starts_with(&FILE_MAGIC)
    }
}

impl<'a> Iterator for FileReader<'a> {
    type Item = Result<&'a [u8], Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if self.rest.is_empty() {
            self.done = true;
            let declared = self.header.block_count;
            if declared != 0 && declared != self.found {
                return Some(Err(Error::CommandCountMismatch {
                    declared,
                    found: self.found,
                }));
            }
            return None;
        }
        // The block's own header says how long it is.
        let len = match StreamHeader::get(&mut Reader::new(self.rest)) {
            Ok(h) if h.magic == crate::stream::STREAM_MAGIC => h.total_len as usize,
            Ok(_) => {
                self.done = true;
                return Some(Err(Error::BadMagic));
            }
            Err(e) => {
                self.done = true;
                return Some(Err(e));
            }
        };
        if len < STREAM_HEADER_SIZE || len > self.rest.len() {
            self.done = true;
            return Some(Err(Error::Truncated));
        }
        let (block, rest) = self.rest.split_at(len);
        match StreamHeader::parse(block) {
            Ok(_) => {
                self.rest = rest;
                self.found += 1;
                Some(Ok(block))
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}
