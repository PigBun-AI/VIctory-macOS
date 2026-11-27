//! `EXECUTE_INDIRECT`: the commands of an application's command signature, read by
//! the GPU from a buffer.
//!
//! The command carries the whole signature (the argument list, the record
//! stride and the root signature it was made for), so the back needs no
//! handle for it: a signature is a few bytes, and a command stream that
//! carries what it uses replays on its own.
//!
//! One record is the arguments of the signature one after the other, packed
//! (no padding), `stride` bytes apart. [`IndirectArgKind`] has the numbers of
//! the application API's indirect argument types, all of them: a back answers `UNSUPPORTED`
//! for those it does not do (draws wait for the render encoder), the numbers
//! stay put.

use crate::cmd::Opcode;
use crate::ids::{ResId, RootSigId};
use crate::status::Error;
use crate::wire::{Reader, Wire, open_enum, wire_struct};
use alloc::vec::Vec;

/// Most arguments one signature has (D3D12 puts no number on it; the
/// limit is for the decoder).
pub const MAX_INDIRECT_ARGS: usize = 32;

open_enum! {
    /// What one argument of a record is. The numbers are those of
    /// The application API's indirect argument type number.
    pub struct IndirectArgKind(u32) {
        /// `{vertex count, instance count, start vertex, start instance}`, 16 bytes. Action.
        DRAW = 0,
        /// `{index count, instance count, start index, base vertex (i32), start instance}`, 20 bytes. Action.
        DRAW_INDEXED = 1,
        /// `{x, y, z}` thread groups, 12 bytes. Action.
        DISPATCH = 2,
        /// A vertex buffer view for slot `index`: `{va, size, stride}`, 16 bytes.
        VERTEX_BUFFER_VIEW = 3,
        /// An index buffer view: `{va, size, format}`, 16 bytes.
        INDEX_BUFFER_VIEW = 4,
        /// `count` 32-bit values for root parameter `index`, from `dest_offset`.
        CONSTANT = 5,
        /// A root CBV of parameter `index`: its GPU address, 8 bytes.
        CONSTANT_BUFFER_VIEW = 6,
        /// A root SRV: its GPU address, 8 bytes.
        SHADER_RESOURCE_VIEW = 7,
        /// A root UAV: its GPU address, 8 bytes.
        UNORDERED_ACCESS_VIEW = 8,
        /// Ray dispatch (56 bytes). Action.
        DISPATCH_RAYS = 9,
        /// `{x, y, z}` mesh thread groups, 12 bytes. Action.
        DISPATCH_MESH = 10,
    }
}

impl IndirectArgKind {
    /// Whether the argument is the command itself (the last of a signature).
    pub const fn is_action(self) -> bool {
        matches!(self.0, 0 | 1 | 2 | 9 | 10)
    }

    /// Whether the argument changes root arguments (the signature needs its root signature).
    pub const fn sets_root(self) -> bool {
        matches!(self.0, 5..=8)
    }

    /// Whether it is a draw or mesh dispatch (graphics bind point).
    pub const fn is_graphics(self) -> bool {
        matches!(self.0, 0 | 1 | 3 | 4 | 10)
    }
}

wire_struct! {
    /// One argument of the signature.
    pub struct IndirectArg {
        kind: IndirectArgKind,
        /// Root parameter index (`CONSTANT`, `*_VIEW` of a root descriptor) or vertex buffer slot.
        index: u32,
        /// `CONSTANT`: first 32-bit value inside the root parameter.
        dest_offset: u32,
        /// `CONSTANT`: number of 32-bit values.
        count: u32,
    }
}

impl IndirectArg {
    /// Bytes the argument takes in a record.
    pub const fn record_bytes(&self) -> u64 {
        match self.kind.0 {
            0 | 3 | 4 => 16,
            1 => 20,
            2 | 10 => 12,
            5 => 4 * self.count as u64,
            6..=8 => 8,
            9 => 56,
            _ => 0,
        }
    }
}

wire_struct! {
    /// Fixed head of `EXECUTE_INDIRECT`; `num_args` [`IndirectArg`]s follow.
    pub struct ExecuteIndirectHead {
        /// The buffer of records.
        args: ResId,
        /// The buffer with the 32-bit count of records, or `NULL` for "`max_count`".
        count: ResId,
        args_offset: u64,
        count_offset: u64,
        /// `MaxCommandCount`.
        max_count: u32,
        /// Bytes from one record to the next.
        stride: u32,
        /// The root signature the signature was made with, `NULL` if it changes no root argument.
        root_sig: RootSigId,
        num_args: u32,
    }
}

/// `EXECUTE_INDIRECT`: `args` is `num_args` encoded [`IndirectArg`]s.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecuteIndirect<'a> {
    pub args: ResId,
    pub args_offset: u64,
    pub count: ResId,
    pub count_offset: u64,
    pub max_count: u32,
    pub stride: u32,
    pub root_sig: RootSigId,
    pub signature: &'a [u8],
}

impl<'a> ExecuteIndirect<'a> {
    /// Builds the command from the signature's arguments, encoding them into `scratch`.
    pub fn encode_signature(args: &[IndirectArg], scratch: &'a mut Vec<u8>) -> &'a [u8] {
        scratch.clear();
        for a in args {
            a.put(scratch);
        }
        scratch
    }

    /// The number of arguments.
    pub fn num_args(&self) -> usize {
        self.signature.len() / IndirectArg::SIZE
    }

    /// The arguments, decoded (the decoder has checked them).
    pub fn arguments(&self) -> impl Iterator<Item = IndirectArg> + '_ {
        self.signature
            .as_chunks::<{ IndirectArg::SIZE }>()
            .0
            .iter()
            .map(|c| IndirectArg::get(&mut Reader::new(c)).unwrap_or(<IndirectArg as Wire>::ZERO))
    }

    /// Bytes of the head and the arguments in the payload.
    pub fn payload_len(&self) -> usize {
        ExecuteIndirectHead::SIZE + self.signature.len()
    }

    pub(crate) fn head(&self) -> ExecuteIndirectHead {
        ExecuteIndirectHead {
            args: self.args,
            count: self.count,
            args_offset: self.args_offset,
            count_offset: self.count_offset,
            max_count: self.max_count,
            stride: self.stride,
            root_sig: self.root_sig,
            num_args: self.num_args() as u32,
        }
    }

    pub(crate) fn from_head(h: ExecuteIndirectHead, signature: &'a [u8]) -> Self {
        ExecuteIndirect {
            args: h.args,
            args_offset: h.args_offset,
            count: h.count,
            count_offset: h.count_offset,
            max_count: h.max_count,
            stride: h.stride,
            root_sig: h.root_sig,
            signature,
        }
    }

    /// The rules a decoder and an encoder enforce (not the buffers' sizes: the back's job).
    pub fn check(&self) -> Result<(), Error> {
        let op = Opcode::EXECUTE_INDIRECT;
        let bad = |field| Error::BadValue {
            opcode: op.0,
            field,
        };
        let n = self.num_args();
        if !self.signature.len().is_multiple_of(IndirectArg::SIZE)
            || n == 0
            || n > MAX_INDIRECT_ARGS
        {
            return Err(bad("num_args"));
        }
        if self.args.is_null() || self.max_count == 0 {
            return Err(bad("args"));
        }
        if !self.args_offset.is_multiple_of(4) || !self.count_offset.is_multiple_of(4) {
            return Err(bad("offset"));
        }
        let mut bytes = 0u64;
        let mut roots = false;
        for (i, a) in self.arguments().enumerate() {
            if !a.kind.is_known() {
                return Err(bad("kind"));
            }
            // The action is the last argument and only one.
            if a.kind.is_action() != (i + 1 == n) {
                return Err(bad("kind"));
            }
            if a.kind == IndirectArgKind::CONSTANT
                && (a.count == 0 || a.count as usize > crate::cmd::MAX_ROOT_CONSTANTS)
            {
                return Err(bad("count"));
            }
            roots |= a.kind.sets_root();
            bytes += a.record_bytes();
        }
        if roots == self.root_sig.is_null() {
            return Err(bad("root_sig"));
        }
        if u64::from(self.stride) < bytes || !self.stride.is_multiple_of(4) {
            return Err(bad("stride"));
        }
        Ok(())
    }
}
