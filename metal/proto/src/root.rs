//! Root signature description: our own POD form, not D3D12's. The front
//! builds it from whatever it was given (or parsed from a serialized root
//! signature); the back derives its argument layout from it.
//!
//! Byte form (`RootSigDesc::encode`): a 16-byte header
//! (`flags`, `param_count`, `range_count`, `sampler_count`, all u32) followed
//! by `param_count` [`RootParam`]s, `range_count` [`DescriptorRange`]s and
//! `sampler_count` [`StaticSampler`]s, each a fixed-size record. Table
//! parameters name a run of the flat range array (`range_start`, `range_count`).

use crate::status::Error;
use crate::types::{RangeKind, RootParamKind, SamplerDesc, ShaderVisibility};
use crate::wire::{Reader, Wire, wire_struct};
use alloc::vec::Vec;

/// A root signature's 32-bit-constant, root-descriptor and table budget in D3D12 is
/// 64 DWORDs, so there are never more than 64 parameters.
pub const MAX_ROOT_PARAMS: usize = 64;
pub const MAX_RANGES: usize = 4096;
pub const MAX_STATIC_SAMPLERS: usize = 2032;
/// A `DescriptorRange::num_descriptors` value meaning "unbounded".
pub const UNBOUNDED: u32 = u32::MAX;
/// A `DescriptorRange::offset_in_table` value meaning "after the previous range".
pub const APPEND: u32 = u32::MAX;

/// `RootSigDesc::flags` bits.
pub mod root_flags {
    /// Shaders index the CBV/SRV/UAV heap directly (`ResourceDescriptorHeap[i]`).
    pub const CBV_SRV_UAV_HEAP_INDEXED: u32 = 1 << 0;
    /// Shaders index the sampler heap directly (`SamplerDescriptorHeap[i]`).
    pub const SAMPLER_HEAP_INDEXED: u32 = 1 << 1;
    pub const ALL_BITS: u32 = 0b11;
}

wire_struct! {
    pub struct RootSigHeader {
        flags: u32,
        param_count: u32,
        range_count: u32,
        sampler_count: u32,
    }
}

wire_struct! {
    /// One root parameter. `num_32bit_values` is used by `CONSTANTS`;
    /// `range_start`/`range_count` by `TABLE`; the registers by every kind
    /// except `TABLE`.
    pub struct RootParam {
        kind: RootParamKind,
        visibility: ShaderVisibility,
        shader_register: u32,
        register_space: u32,
        num_32bit_values: u32,
        range_start: u32,
        range_count: u32,
        _pad: u32,
    }
}

wire_struct! {
    pub struct DescriptorRange {
        kind: RangeKind,
        /// [`UNBOUNDED`] for an open-ended range.
        num_descriptors: u32,
        base_register: u32,
        register_space: u32,
        /// Slot in the table where the range starts, or [`APPEND`].
        offset_in_table: u32,
        flags: u32,
    }
}

wire_struct! {
    pub struct StaticSampler {
        sampler: SamplerDesc,
        shader_register: u32,
        register_space: u32,
        visibility: ShaderVisibility,
        _pad: u32,
    }
}

/// An owned root signature description.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RootSigDesc {
    pub flags: u32,
    pub params: Vec<RootParam>,
    pub ranges: Vec<DescriptorRange>,
    pub static_samplers: Vec<StaticSampler>,
}

const OPCODE_NONE: u16 = 0;

impl RootSigDesc {
    /// Bytes the encoded form takes.
    pub fn encoded_len(&self) -> usize {
        RootSigHeader::SIZE
            + self.params.len() * RootParam::SIZE
            + self.ranges.len() * DescriptorRange::SIZE
            + self.static_samplers.len() * StaticSampler::SIZE
    }

    /// The byte form `CreateRootSignature` takes. Fails when a count is over its limit.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.params.len() > MAX_ROOT_PARAMS
            || self.ranges.len() > MAX_RANGES
            || self.static_samplers.len() > MAX_STATIC_SAMPLERS
        {
            return Err(Error::TooLarge);
        }
        let mut out = Vec::with_capacity(self.encoded_len());
        RootSigHeader {
            flags: self.flags,
            param_count: self.params.len() as u32,
            range_count: self.ranges.len() as u32,
            sampler_count: self.static_samplers.len() as u32,
        }
        .put(&mut out);
        for p in &self.params {
            p.put(&mut out);
        }
        for r in &self.ranges {
            r.put(&mut out);
        }
        for s in &self.static_samplers {
            s.put(&mut out);
        }
        Ok(out)
    }

    /// Parses and checks the byte form: counts within limits and within the
    /// bytes, known enums, tables inside the range array, nothing left over.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        let h = RootSigHeader::get(&mut r)?;
        let (np, nr, ns) = (
            h.param_count as usize,
            h.range_count as usize,
            h.sampler_count as usize,
        );
        let bad = |field| Error::BadValue {
            opcode: OPCODE_NONE,
            field,
        };
        if np > MAX_ROOT_PARAMS || nr > MAX_RANGES || ns > MAX_STATIC_SAMPLERS {
            return Err(bad("count"));
        }
        if h.flags & !root_flags::ALL_BITS != 0 {
            return Err(bad("flags"));
        }
        // The counts are bounded above, so this cannot overflow; check them
        // against the bytes before allocating anything.
        let need = np * RootParam::SIZE + nr * DescriptorRange::SIZE + ns * StaticSampler::SIZE;
        if need > r.remaining() {
            return Err(Error::Truncated);
        }
        if need < r.remaining() {
            return Err(Error::TrailingBytes);
        }
        let mut params = Vec::with_capacity(np);
        for _ in 0..np {
            let p = RootParam::get(&mut r)?;
            if !p.kind.is_known() || !p.visibility.is_known() {
                return Err(bad("param"));
            }
            if p.kind == RootParamKind::TABLE {
                let end = (p.range_start as usize).checked_add(p.range_count as usize);
                if end.is_none_or(|e| e > nr) {
                    return Err(bad("table range"));
                }
            }
            params.push(p);
        }
        let mut ranges = Vec::with_capacity(nr);
        for _ in 0..nr {
            let d = DescriptorRange::get(&mut r)?;
            if !d.kind.is_known() {
                return Err(bad("range kind"));
            }
            ranges.push(d);
        }
        let mut static_samplers = Vec::with_capacity(ns);
        for _ in 0..ns {
            let s = StaticSampler::get(&mut r)?;
            if !s.visibility.is_known() {
                return Err(bad("sampler visibility"));
            }
            static_samplers.push(s);
        }
        Ok(RootSigDesc {
            flags: h.flags,
            params,
            ranges,
            static_samplers,
        })
    }
}
