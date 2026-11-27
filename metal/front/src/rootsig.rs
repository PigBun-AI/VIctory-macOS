//! Root signatures: description types, `D3D12SerializeRootSignature` /
//! `D3D12SerializeVersionedRootSignature` and the deserializers.
//!
//! The blob is a DXBC container with one `RTS0` part:
//!
//! ```text
//! 'DXBC' | digest[16] | 1u32 | total_size | 1u32 (parts) | 36u32 (offset)
//! 'RTS0' | part_size | body
//! body: version(1|2) num_params params_off(24) num_samplers samplers_off flags
//!       params[num_params] { type, visibility, payload_off }   (12 bytes each)
//!       payload of each parameter, in order:
//!         table      { num_ranges, ranges_off } then its ranges (20 bytes in 1.0,
//!                      24 with flags in 1.1)
//!         constants  { reg, space, num_values }
//!         descriptor { reg, space [, flags in 1.1] }
//!       static samplers, 52 bytes each
//! ```
//!
//! All offsets in the body count from the start of the body. The digest is
//! MD5-like with Microsoft's padding over everything after it. Verified
//! byte for byte against D3DMetal on Wine 11 (see
//! `tests/golden/d3dmetal-wine11.txt`); note that is D3DMetal's output, not
//! necessarily real Windows' (no Windows machine was available).

use crate::hresult::{Hr, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V1_0,
    V1_1,
}

impl Version {
    fn body_version(self) -> u32 {
        match self {
            Version::V1_0 => 1,
            Version::V1_1 => 2,
        }
    }
}

// D3D12_ROOT_PARAMETER_TYPE
pub const PARAM_TABLE: u32 = 0;
pub const PARAM_CONSTANTS: u32 = 1;
pub const PARAM_CBV: u32 = 2;
pub const PARAM_SRV: u32 = 3;
pub const PARAM_UAV: u32 = 4;

// D3D12_DESCRIPTOR_RANGE_TYPE
pub const RANGE_SRV: u32 = 0;
pub const RANGE_UAV: u32 = 1;
pub const RANGE_CBV: u32 = 2;
pub const RANGE_SAMPLER: u32 = 3;

/// Flags 1.0 ranges / root descriptors get when converted to 1.1:
/// DESCRIPTORS_VOLATILE | DATA_VOLATILE, and DATA_VOLATILE.
pub const RANGE_FLAGS_FROM_1_0: u32 = 0x3;
pub const ROOT_DESCRIPTOR_FLAGS_FROM_1_0: u32 = 0x2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub ty: u32,
    pub num: u32,
    pub base: u32,
    pub space: u32,
    /// 1.1 only; ignored (and zero) in 1.0.
    pub flags: u32,
    pub offset: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Table(Vec<Range>),
    Constants { reg: u32, space: u32, num: u32 },
    /// `ty` is `PARAM_CBV`, `PARAM_SRV` or `PARAM_UAV`; `flags` is 1.1 only.
    Descriptor { ty: u32, reg: u32, space: u32, flags: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub kind: ParamKind,
    pub visibility: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StaticSampler {
    pub filter: u32,
    pub address_u: u32,
    pub address_v: u32,
    pub address_w: u32,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border: u32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub reg: u32,
    pub space: u32,
    pub visibility: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RootSignatureDesc {
    pub version: Version,
    pub params: Vec<Param>,
    pub samplers: Vec<StaticSampler>,
    pub flags: u32,
}

impl RootSignatureDesc {
    /// Convert between versions the way D3D12 does: 1.0 → 1.1 gives every
    /// range DESCRIPTORS_VOLATILE | DATA_VOLATILE and every root descriptor
    /// DATA_VOLATILE; 1.1 → 1.0 drops the flags.
    pub fn to_version(&self, version: Version) -> RootSignatureDesc {
        let mut d = self.clone();
        if self.version == version {
            return d;
        }
        d.version = version;
        for p in &mut d.params {
            match &mut p.kind {
                ParamKind::Table(ranges) => {
                    for r in ranges {
                        r.flags = if version == Version::V1_1 { RANGE_FLAGS_FROM_1_0 } else { 0 };
                    }
                }
                ParamKind::Descriptor { flags, .. } => {
                    *flags = if version == Version::V1_1 { ROOT_DESCRIPTOR_FLAGS_FROM_1_0 } else { 0 };
                }
                ParamKind::Constants { .. } => {}
            }
        }
        d
    }
}

const ROOT_FLAGS_MASK: u32 = 0xFFF;
const MAX_VISIBILITY: u32 = 7;

fn validate(d: &RootSignatureDesc) -> Result<()> {
    if d.flags & !ROOT_FLAGS_MASK != 0 {
        return Err(Hr::E_INVALIDARG);
    }
    for p in &d.params {
        if p.visibility > MAX_VISIBILITY {
            return Err(Hr::E_INVALIDARG);
        }
        match &p.kind {
            ParamKind::Table(ranges) => {
                if ranges.iter().any(|r| r.ty > RANGE_SAMPLER) {
                    return Err(Hr::E_INVALIDARG);
                }
            }
            ParamKind::Descriptor { ty, .. } if !matches!(*ty, PARAM_CBV | PARAM_SRV | PARAM_UAV) => {
                return Err(Hr::E_INVALIDARG);
            }
            _ => {}
        }
    }
    if d.samplers.iter().any(|s| s.visibility > MAX_VISIBILITY) {
        return Err(Hr::E_INVALIDARG);
    }
    Ok(())
}

struct W(Vec<u8>);

impl W {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
}

/// The `RTS0` part body.
fn serialize_body(d: &RootSignatureDesc) -> Vec<u8> {
    let v11 = d.version == Version::V1_1;
    let mut w = W(Vec::new());
    w.u32(d.version.body_version());
    w.u32(d.params.len() as u32);
    w.u32(24);
    w.u32(d.samplers.len() as u32);
    w.u32(0); // samplers offset, patched below
    w.u32(d.flags);
    // Parameter headers with their payload offsets, payloads after them.
    let mut payload_off = 24 + 12 * d.params.len() as u32;
    let mut payloads = W(Vec::new());
    for p in &d.params {
        let (ty, size) = match &p.kind {
            ParamKind::Table(r) => {
                (PARAM_TABLE, 8 + r.len() as u32 * if v11 { 24 } else { 20 })
            }
            ParamKind::Constants { .. } => (PARAM_CONSTANTS, 12),
            ParamKind::Descriptor { ty, .. } => (*ty, if v11 { 12 } else { 8 }),
        };
        w.u32(ty);
        w.u32(p.visibility);
        w.u32(payload_off);
        match &p.kind {
            ParamKind::Table(ranges) => {
                payloads.u32(ranges.len() as u32);
                payloads.u32(payload_off + 8);
                for r in ranges {
                    payloads.u32(r.ty);
                    payloads.u32(r.num);
                    payloads.u32(r.base);
                    payloads.u32(r.space);
                    if v11 {
                        payloads.u32(r.flags);
                    }
                    payloads.u32(r.offset);
                }
            }
            ParamKind::Constants { reg, space, num } => {
                payloads.u32(*reg);
                payloads.u32(*space);
                payloads.u32(*num);
            }
            ParamKind::Descriptor { reg, space, flags, .. } => {
                payloads.u32(*reg);
                payloads.u32(*space);
                if v11 {
                    payloads.u32(*flags);
                }
            }
        }
        payload_off += size;
    }
    w.0.extend_from_slice(&payloads.0);
    // Patch the samplers offset.
    let samplers_off = w.0.len() as u32;
    w.0[16..20].copy_from_slice(&samplers_off.to_le_bytes());
    for s in &d.samplers {
        w.u32(s.filter);
        w.u32(s.address_u);
        w.u32(s.address_v);
        w.u32(s.address_w);
        w.f32(s.mip_lod_bias);
        w.u32(s.max_anisotropy);
        w.u32(s.comparison);
        w.u32(s.border);
        w.f32(s.min_lod);
        w.f32(s.max_lod);
        w.u32(s.reg);
        w.u32(s.space);
        w.u32(s.visibility);
    }
    w.0
}

/// `D3D12SerializeRootSignature` / `D3D12SerializeVersionedRootSignature`.
pub fn serialize(d: &RootSignatureDesc) -> Result<Vec<u8>> {
    validate(d)?;
    let body = serialize_body(d);
    let total = 36 + 8 + body.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"DXBC");
    out.extend_from_slice(&[0; 16]);
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&36u32.to_le_bytes());
    out.extend_from_slice(b"RTS0");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    let digest = dxbc_digest(&out[20..]);
    out[4..20].copy_from_slice(&digest);
    Ok(out)
}

struct R<'a> {
    b: &'a [u8],
}

impl R<'_> {
    fn u32(&self, at: usize) -> Result<u32> {
        let s = self.b.get(at..at.checked_add(4).ok_or(Hr::E_INVALIDARG)?).ok_or(Hr::E_INVALIDARG)?;
        Ok(u32::from_le_bytes(s.try_into().unwrap()))
    }
    fn f32(&self, at: usize) -> Result<f32> {
        Ok(f32::from_bits(self.u32(at)?))
    }
}

/// `D3D12CreateRootSignatureDeserializer` and its versioned twin: parse a
/// blob. Malformed input is `E_INVALIDARG` (D3DMetal crashes on it).
pub fn deserialize(blob: &[u8]) -> Result<RootSignatureDesc> {
    let c = R { b: blob };
    if blob.len() < 36 || &blob[..4] != b"DXBC" {
        return Err(Hr::E_INVALIDARG);
    }
    if c.u32(24)? as usize != blob.len() {
        return Err(Hr::E_INVALIDARG);
    }
    let parts = c.u32(28)? as usize;
    if parts > (blob.len() - 32) / 4 {
        return Err(Hr::E_INVALIDARG);
    }
    let mut body = None;
    for i in 0..parts {
        let off = c.u32(32 + 4 * i)? as usize;
        if blob.get(off..off.saturating_add(8)).is_none() {
            return Err(Hr::E_INVALIDARG);
        }
        if &blob[off..off + 4] == b"RTS0" {
            let size = c.u32(off + 4)? as usize;
            body = Some(blob.get(off + 8..off.checked_add(8 + size).ok_or(Hr::E_INVALIDARG)?).ok_or(Hr::E_INVALIDARG)?);
            break;
        }
    }
    let r = R { b: body.ok_or(Hr::E_INVALIDARG)? };
    let version = match r.u32(0)? {
        1 => Version::V1_0,
        2 => Version::V1_1,
        _ => return Err(Hr::E_INVALIDARG),
    };
    let v11 = version == Version::V1_1;
    let nparams = r.u32(4)? as usize;
    let params_off = r.u32(8)? as usize;
    let nsamplers = r.u32(12)? as usize;
    let samplers_off = r.u32(16)? as usize;
    let flags = r.u32(20)?;
    // Every count is bounded by the bytes that must back it.
    if nparams > r.b.len() / 12 || nsamplers > r.b.len() / 52 {
        return Err(Hr::E_INVALIDARG);
    }
    let mut params = Vec::with_capacity(nparams);
    for i in 0..nparams {
        let h = params_off.checked_add(12 * i).ok_or(Hr::E_INVALIDARG)?;
        let ty = r.u32(h)?;
        let visibility = r.u32(h + 4)?;
        let p = r.u32(h + 8)? as usize;
        let kind = match ty {
            PARAM_TABLE => {
                let n = r.u32(p)? as usize;
                let ro = r.u32(p.checked_add(4).ok_or(Hr::E_INVALIDARG)?)? as usize;
                let rsize = if v11 { 24 } else { 20 };
                if n > r.b.len() / rsize {
                    return Err(Hr::E_INVALIDARG);
                }
                let mut ranges = Vec::with_capacity(n);
                for j in 0..n {
                    let a = ro.checked_add(rsize * j).ok_or(Hr::E_INVALIDARG)?;
                    ranges.push(Range {
                        ty: r.u32(a)?,
                        num: r.u32(a + 4)?,
                        base: r.u32(a + 8)?,
                        space: r.u32(a + 12)?,
                        flags: if v11 { r.u32(a + 16)? } else { 0 },
                        offset: r.u32(a + if v11 { 20 } else { 16 })?,
                    });
                }
                ParamKind::Table(ranges)
            }
            PARAM_CONSTANTS => ParamKind::Constants { reg: r.u32(p)?, space: r.u32(p + 4)?, num: r.u32(p + 8)? },
            PARAM_CBV | PARAM_SRV | PARAM_UAV => ParamKind::Descriptor {
                ty,
                reg: r.u32(p)?,
                space: r.u32(p + 4)?,
                flags: if v11 { r.u32(p + 8)? } else { 0 },
            },
            _ => return Err(Hr::E_INVALIDARG),
        };
        params.push(Param { kind, visibility });
    }
    let mut samplers = Vec::with_capacity(nsamplers);
    for i in 0..nsamplers {
        let a = samplers_off.checked_add(52 * i).ok_or(Hr::E_INVALIDARG)?;
        samplers.push(StaticSampler {
            filter: r.u32(a)?,
            address_u: r.u32(a + 4)?,
            address_v: r.u32(a + 8)?,
            address_w: r.u32(a + 12)?,
            mip_lod_bias: r.f32(a + 16)?,
            max_anisotropy: r.u32(a + 20)?,
            comparison: r.u32(a + 24)?,
            border: r.u32(a + 28)?,
            min_lod: r.f32(a + 32)?,
            max_lod: r.f32(a + 36)?,
            reg: r.u32(a + 40)?,
            space: r.u32(a + 44)?,
            visibility: r.u32(a + 48)?,
        });
    }
    Ok(RootSignatureDesc { version, params, samplers, flags })
}

// ---- the DXBC digest -----------------------------------------------------

/// MD5 with the container's own padding: whole 64-byte blocks first, then a
/// final block (or two) holding the tail with the bit count in front of it
/// and `(bytes * 2) | 1` at the end. Input is everything after the digest.
pub fn dxbc_digest(data: &[u8]) -> [u8; 16] {
    let n = data.len();
    let left = n & 63;
    let full = n - left;
    let mut st: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];
    for b in data[..full].chunks_exact(64) {
        md5_block(&mut st, b);
    }
    let bits = (n as u32).wrapping_mul(8);
    let or2 = (n as u32).wrapping_mul(2) | 1;
    let tail = &data[full..];
    let mut blk = [0u8; 64];
    if left < 56 {
        blk[..4].copy_from_slice(&bits.to_le_bytes());
        blk[4..4 + left].copy_from_slice(tail);
        blk[4 + left] = 0x80;
        blk[60..].copy_from_slice(&or2.to_le_bytes());
        md5_block(&mut st, &blk);
    } else {
        blk[..left].copy_from_slice(tail);
        blk[left] = 0x80;
        md5_block(&mut st, &blk);
        let mut blk2 = [0u8; 64];
        blk2[..4].copy_from_slice(&bits.to_le_bytes());
        blk2[60..].copy_from_slice(&or2.to_le_bytes());
        md5_block(&mut st, &blk2);
    }
    let mut out = [0u8; 16];
    for (i, v) in st.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15,
    21,
];

fn md5_k(i: usize) -> u32 {
    // floor(2^32 * |sin(i + 1)|)
    ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32
}

fn md5_block(st: &mut [u32; 4], b: &[u8]) {
    let mut m = [0u32; 16];
    for (i, c) in b.chunks_exact(4).enumerate() {
        m[i] = u32::from_le_bytes(c.try_into().unwrap());
    }
    let [mut a, mut bb, mut c, mut d] = *st;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((bb & c) | (!bb & d), i),
            1 => ((d & bb) | (!d & c), (5 * i + 1) % 16),
            2 => (bb ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (bb | !d), (7 * i) % 16),
        };
        let f = f.wrapping_add(a).wrapping_add(md5_k(i)).wrapping_add(m[g]);
        a = d;
        d = c;
        c = bb;
        bb = bb.wrapping_add(f.rotate_left(MD5_S[i]));
    }
    st[0] = st[0].wrapping_add(a);
    st[1] = st[1].wrapping_add(bb);
    st[2] = st[2].wrapping_add(c);
    st[3] = st[3].wrapping_add(d);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    /// (name, blob) of every serialize line in the D3DMetal run that
    /// succeeded.
    fn golden() -> Vec<(String, Vec<u8>)> {
        let text = include_str!("../tests/golden/d3dmetal-wine11.txt");
        text.lines()
            .filter_map(|l| {
                let l = l.strip_prefix("GOLD serialize ")?;
                let (name, rest) = l.split_once(": S_OK ")?;
                let hex = rest.split(' ').next()?;
                Some((name.to_string(), unhex(hex)))
            })
            .collect()
    }

    fn sampler(n: u32) -> StaticSampler {
        if n == 0 {
            StaticSampler {
                filter: 21,
                address_u: 1,
                address_v: 3,
                address_w: 4,
                mip_lod_bias: 0.5,
                max_anisotropy: 16,
                comparison: 4,
                border: 2,
                min_lod: 0.0,
                max_lod: 12.0,
                reg: 2,
                space: 5,
                visibility: 5,
            }
        } else {
            StaticSampler {
                filter: 85,
                address_u: 2,
                address_v: 5,
                address_w: 1,
                mip_lod_bias: -1.0,
                max_anisotropy: 4,
                comparison: 1,
                border: 0,
                min_lod: 1.0,
                max_lod: f32::MAX,
                reg: 0,
                space: 0,
                visibility: 0,
            }
        }
    }

    fn range(ty: u32, num: u32, base: u32, space: u32, flags: u32, offset: u32) -> Range {
        Range { ty, num, base, space, flags, offset }
    }

    const APPEND: u32 = 0xffff_ffff;

    fn ranges(v11: bool) -> Vec<Range> {
        let f = |a, b, c| if v11 { (a, b, c) } else { (0, 0, 0) };
        vec![
            range(RANGE_SRV, 2, 0, 0, 0, APPEND),
            range(RANGE_UAV, 4, 1, 3, f(3, 0, 0).0, 8),
            range(RANGE_SAMPLER, u32::MAX, 0, 1, if v11 { 1 } else { 0 }, APPEND),
        ]
    }

    fn view(ty: u32, reg: u32, space: u32, flags: u32, vis: u32) -> Param {
        Param { kind: ParamKind::Descriptor { ty, reg, space, flags }, visibility: vis }
    }
    fn consts(reg: u32, space: u32, num: u32, vis: u32) -> Param {
        Param { kind: ParamKind::Constants { reg, space, num }, visibility: vis }
    }
    fn table(r: Vec<Range>, vis: u32) -> Param {
        Param { kind: ParamKind::Table(r), visibility: vis }
    }

    /// The descriptions smoke.rs serializes, by golden name.
    fn cases() -> Vec<(&'static str, RootSignatureDesc)> {
        let d10 = |params, samplers, flags| RootSignatureDesc { version: Version::V1_0, params, samplers, flags };
        let d11 = |params, samplers, flags| RootSignatureDesc { version: Version::V1_1, params, samplers, flags };
        let mixed10 = vec![
            table(ranges(false), 0),
            consts(0, 7, 4, 0),
            view(PARAM_CBV, 1, 0, 0, 1),
            view(PARAM_SRV, 2, 9, 0, 0),
            view(PARAM_UAV, 3, 9, 0, 0),
        ];
        let mixed11 = vec![
            table(ranges(true), 0),
            consts(0, 7, 4, 0),
            view(PARAM_CBV, 1, 0, 0, 1),
            view(PARAM_SRV, 2, 9, 8, 0),
            view(PARAM_UAV, 3, 9, 2, 0),
        ];
        let host11 = vec![
            consts(0, 7, 8, 0),
            view(PARAM_CBV, 0, 6, 0, 0),
            view(PARAM_SRV, 0, 6, 0, 0),
            view(PARAM_UAV, 0, 6, 0, 0),
        ];
        let host_flags = 0x1 | 0x400 | 0x800;
        let two10 = vec![
            table(ranges(false)[..2].to_vec(), 5),
            consts(3, 1, 2, 4),
            table(ranges(false)[1..].to_vec(), 6),
            view(PARAM_UAV, 5, 5, 0, 7),
        ];
        let two11 = vec![
            table(ranges(true)[..2].to_vec(), 5),
            consts(3, 1, 2, 4),
            table(ranges(true)[1..].to_vec(), 6),
            view(PARAM_UAV, 5, 5, 4, 7),
        ];
        vec![
            ("1.0 desc empty -> 1_0", d10(vec![], vec![], 1)),
            ("1.0 desc empty-none -> 1_0", d10(vec![], vec![], 0)),
            ("1.0 desc mixed -> 1_0", d10(mixed10.clone(), vec![sampler(0), sampler(1)], 0x4)),
            ("1.0 desc two-tables -> 1_0", d10(two10.clone(), vec![], 0x1 | 0x20)),
            ("1.0 desc constants-only -> 1_0", d10(vec![mixed10[1].clone()], vec![], 0)),
            ("1.0 desc samplers-only -> 1_0", d10(vec![], vec![sampler(0), sampler(1)], 0)),
            ("versioned 1.1 desc mixed", d11(mixed11, vec![sampler(0), sampler(1)], 0x4)),
            ("versioned 1.1 desc two-tables", d11(two11, vec![sampler(1)], 0x80)),
            ("versioned 1.1 desc host", d11(host11.clone(), vec![], host_flags)),
            ("versioned 1.1 desc host-with-sampler", d11(host11, vec![sampler(0)], host_flags)),
            ("versioned 1.0 desc mixed", d10(mixed10, vec![sampler(0), sampler(1)], 0x4)),
        ]
    }

    #[test]
    fn digest_matches_every_golden_blob() {
        let g = golden();
        assert!(g.len() >= 16);
        for (name, blob) in &g {
            assert_eq!(&dxbc_digest(&blob[20..]), &blob[4..20], "{name}");
        }
    }

    #[test]
    fn serialization_is_byte_identical_to_d3dmetal() {
        let g = golden();
        for (name, desc) in cases() {
            let want = &g.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no golden {name}")).1;
            let got = serialize(&desc).unwrap();
            assert_eq!(&got, want, "{name}");
        }
    }

    #[test]
    fn every_golden_blob_round_trips() {
        for (name, blob) in golden() {
            let d = deserialize(&blob).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(serialize(&d).unwrap(), blob, "{name}");
        }
    }

    #[test]
    fn version_conversion_matches_d3dmetal() {
        // "versioned deserialize v10-mixed at 1.1": every range 0x3, root
        // descriptors 0x2.
        let g = golden();
        let blob = &g.iter().find(|(n, _)| n == "versioned 1.0 desc mixed").unwrap().1;
        let up = deserialize(blob).unwrap().to_version(Version::V1_1);
        match &up.params[0].kind {
            ParamKind::Table(r) => assert!(r.iter().all(|r| r.flags == 3)),
            _ => panic!(),
        }
        assert!(matches!(up.params[3].kind, ParamKind::Descriptor { flags: 2, .. }));
        // And back down drops them again.
        let down = up.to_version(Version::V1_0);
        assert_eq!(serialize(&down).unwrap(), *blob);
    }

    #[test]
    fn malformed_blobs_are_refused_not_crashed_on() {
        let g = golden();
        let blob = &g.iter().find(|(n, _)| n == "versioned 1.1 desc mixed").unwrap().1;
        assert_eq!(deserialize(&[]), Err(Hr::E_INVALIDARG));
        assert_eq!(deserialize(&[0xff; 64]), Err(Hr::E_INVALIDARG));
        // Every truncation.
        for n in 0..blob.len() {
            assert!(deserialize(&blob[..n]).is_err(), "truncated to {n}");
        }
        // Every single-byte corruption must not panic.
        for i in 0..blob.len() {
            let mut b = blob.clone();
            b[i] ^= 0xff;
            let _ = deserialize(&b);
        }
        // Huge counts.
        let mut b = blob.clone();
        b[0x2c + 4..0x2c + 8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(deserialize(&b).is_err());
    }

    #[test]
    fn bad_descriptions_are_refused() {
        let mut d = RootSignatureDesc { version: Version::V1_0, params: vec![], samplers: vec![], flags: 0 };
        d.flags = 0x10000;
        assert_eq!(serialize(&d), Err(Hr::E_INVALIDARG));
        d.flags = 0;
        d.params.push(consts(0, 0, 1, 99));
        assert_eq!(serialize(&d), Err(Hr::E_INVALIDARG));
    }
}
