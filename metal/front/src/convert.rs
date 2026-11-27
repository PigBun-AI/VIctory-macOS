//! D3D12 descriptions → proto descriptions. Pure functions; the only place
//! that knows both numberings.

use victory_metal_proto::root::{self, DescriptorRange, RootParam, RootSigDesc, StaticSampler};
use victory_metal_proto::types::{
    AddressMode, CompareFunc, Filter, RangeKind, Reduction, RootParamKind, SamplerDesc,
    ShaderVisibility,
};

use crate::hresult::{Hr, Result};
use crate::rootsig::{self, ParamKind, RootSignatureDesc};

/// `D3D12_ROOT_SIGNATURE_FLAG_*` bits that proto carries.
const FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED: u32 = 0x400;
const FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED: u32 = 0x800;

/// `D3D12_SHADER_VISIBILITY`: hull, domain and geometry have no proto
/// equivalent (M1 and M3 have no such stages).
pub fn visibility(v: u32) -> Result<ShaderVisibility> {
    Ok(match v {
        0 => ShaderVisibility::ALL,
        1 => ShaderVisibility::VERTEX,
        5 => ShaderVisibility::FRAGMENT,
        6 => ShaderVisibility::OBJECT,
        7 => ShaderVisibility::MESH,
        2..=4 => return Err(Hr::E_NOTIMPL),
        _ => return Err(Hr::E_INVALIDARG),
    })
}

/// `D3D12_TEXTURE_ADDRESS_MODE` (1-based).
pub fn address_mode(m: u32) -> Result<AddressMode> {
    match m {
        1..=5 => Ok(AddressMode(m as u8 - 1)),
        _ => Err(Hr::E_INVALIDARG),
    }
}

/// `D3D12_COMPARISON_FUNC` (1-based).
pub fn compare_func(f: u32) -> Result<CompareFunc> {
    match f {
        1..=8 => Ok(CompareFunc(f as u8 - 1)),
        _ => Err(Hr::E_INVALIDARG),
    }
}

/// `D3D12_FILTER`: bit 0 mip, bit 2 mag, bit 4 min, bits 7-8 reduction;
/// the anisotropic filters (bit 6) have every bit set to linear.
pub fn filter(f: u32) -> Result<(Filter, Filter, Filter, Reduction)> {
    if f & !0x1d5 != 0 {
        return Err(Hr::E_INVALIDARG);
    }
    let bit = |b: u32| {
        if f >> b & 1 == 1 {
            Filter::LINEAR
        } else {
            Filter::NEAREST
        }
    };
    Ok((bit(4), bit(2), bit(0), Reduction((f >> 7 & 3) as u8)))
}

fn border(c: u32) -> Result<[f32; 4]> {
    Ok(match c {
        0 => [0.0; 4],
        1 => [0.0, 0.0, 0.0, 1.0],
        2 => [1.0; 4],
        _ => return Err(Hr::E_INVALIDARG),
    })
}

pub struct SamplerParams {
    pub filter: u32,
    pub address_u: u32,
    pub address_v: u32,
    pub address_w: u32,
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border_color: [f32; 4],
    pub min_lod: f32,
    pub max_lod: f32,
}

pub fn sampler(s: &SamplerParams) -> Result<SamplerDesc> {
    let (min_filter, mag_filter, mip_filter, reduction) = filter(s.filter)?;
    // The comparison function only counts for comparison filters.
    let compare = if reduction == Reduction::COMPARISON {
        compare_func(s.comparison)?
    } else {
        CompareFunc::NEVER
    };
    Ok(SamplerDesc {
        min_filter,
        mag_filter,
        mip_filter,
        reduction,
        address_u: address_mode(s.address_u)?,
        address_v: address_mode(s.address_v)?,
        address_w: address_mode(s.address_w)?,
        compare,
        mip_lod_bias: s.mip_lod_bias,
        min_lod: s.min_lod,
        max_lod: s.max_lod,
        // MaxAnisotropy only counts for the anisotropic filters (D3D12_FILTER
        // bit 6); every application fills it in (16 is common) for the others,
        // and proto reads "more than 1" as anisotropic.
        max_anisotropy: if s.filter & 0x40 != 0 {
            s.max_anisotropy.clamp(1, 16)
        } else {
            1
        },
        border_color: s.border_color,
    })
}

/// A deserialized (any version) root signature → proto's.
pub fn root_signature(d: &RootSignatureDesc) -> Result<RootSigDesc> {
    let d = d.to_version(rootsig::Version::V1_1);
    let mut out = RootSigDesc::default();
    if d.flags & FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED != 0 {
        out.flags |= root::root_flags::CBV_SRV_UAV_HEAP_INDEXED;
    }
    if d.flags & FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED != 0 {
        out.flags |= root::root_flags::SAMPLER_HEAP_INDEXED;
    }
    for p in &d.params {
        let mut rp = RootParam {
            kind: RootParamKind::CONSTANTS,
            visibility: visibility(p.visibility)?,
            shader_register: 0,
            register_space: 0,
            num_32bit_values: 0,
            range_start: 0,
            range_count: 0,
            _pad: 0,
        };
        match &p.kind {
            ParamKind::Constants { reg, space, num } => {
                rp.kind = RootParamKind::CONSTANTS;
                rp.shader_register = *reg;
                rp.register_space = *space;
                rp.num_32bit_values = *num;
            }
            ParamKind::Descriptor { ty, reg, space, .. } => {
                rp.kind = match *ty {
                    rootsig::PARAM_CBV => RootParamKind::CBV,
                    rootsig::PARAM_SRV => RootParamKind::SRV,
                    rootsig::PARAM_UAV => RootParamKind::UAV,
                    _ => return Err(Hr::E_INVALIDARG),
                };
                rp.shader_register = *reg;
                rp.register_space = *space;
            }
            ParamKind::Table(ranges) => {
                rp.kind = RootParamKind::TABLE;
                rp.range_start = out.ranges.len() as u32;
                rp.range_count = ranges.len() as u32;
                for r in ranges {
                    if r.ty > rootsig::RANGE_SAMPLER {
                        return Err(Hr::E_INVALIDARG);
                    }
                    out.ranges.push(DescriptorRange {
                        kind: RangeKind(r.ty),
                        num_descriptors: r.num,
                        base_register: r.base,
                        register_space: r.space,
                        offset_in_table: r.offset,
                        flags: r.flags,
                    });
                }
            }
        }
        out.params.push(rp);
    }
    for s in &d.samplers {
        out.static_samplers.push(StaticSampler {
            sampler: sampler(&SamplerParams {
                filter: s.filter,
                address_u: s.address_u,
                address_v: s.address_v,
                address_w: s.address_w,
                mip_lod_bias: s.mip_lod_bias,
                max_anisotropy: s.max_anisotropy,
                comparison: s.comparison,
                border_color: border(s.border)?,
                min_lod: s.min_lod,
                max_lod: s.max_lod,
            })?,
            shader_register: s.reg,
            register_space: s.space,
            visibility: visibility(s.visibility)?,
            _pad: 0,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rootsig::{Param, Range};

    #[test]
    fn filters_decode() {
        // MIN_MAG_MIP_POINT, MIN_MAG_MIP_LINEAR, ANISOTROPIC, COMPARISON_MIN_MAG_MIP_LINEAR, MAXIMUM_MIN_MAG_MIP_POINT
        let f = |v| filter(v).unwrap();
        assert_eq!(
            f(0),
            (
                Filter::NEAREST,
                Filter::NEAREST,
                Filter::NEAREST,
                Reduction::STANDARD
            )
        );
        assert_eq!(
            f(0x15),
            (
                Filter::LINEAR,
                Filter::LINEAR,
                Filter::LINEAR,
                Reduction::STANDARD
            )
        );
        assert_eq!(f(0x55).0, Filter::LINEAR);
        assert_eq!(f(0x95).3, Reduction::COMPARISON);
        assert_eq!(f(0x180).3, Reduction::MAX);
        assert_eq!(
            f(0x14),
            (
                Filter::LINEAR,
                Filter::LINEAR,
                Filter::NEAREST,
                Reduction::STANDARD
            )
        );
        assert_eq!(filter(0x2), Err(Hr::E_INVALIDARG));
    }

    #[test]
    fn root_signature_maps_to_proto() {
        let d = RootSignatureDesc {
            version: rootsig::Version::V1_1,
            params: vec![
                Param {
                    kind: ParamKind::Constants {
                        reg: 0,
                        space: 0,
                        num: 4,
                    },
                    visibility: 0,
                },
                Param {
                    kind: ParamKind::Descriptor {
                        ty: rootsig::PARAM_UAV,
                        reg: 1,
                        space: 2,
                        flags: 0,
                    },
                    visibility: 0,
                },
                Param {
                    kind: ParamKind::Table(vec![
                        Range {
                            ty: rootsig::RANGE_SRV,
                            num: 4,
                            base: 0,
                            space: 0,
                            flags: 0,
                            offset: u32::MAX,
                        },
                        Range {
                            ty: rootsig::RANGE_UAV,
                            num: u32::MAX,
                            base: 0,
                            space: 1,
                            flags: 0,
                            offset: 4,
                        },
                    ]),
                    visibility: 0,
                },
            ],
            samplers: vec![],
            flags: 0x400,
        };
        let p = root_signature(&d).unwrap();
        assert_eq!(p.flags, root::root_flags::CBV_SRV_UAV_HEAP_INDEXED);
        assert_eq!(p.params.len(), 3);
        assert_eq!(p.params[0].kind, RootParamKind::CONSTANTS);
        assert_eq!(p.params[0].num_32bit_values, 4);
        assert_eq!(p.params[1].kind, RootParamKind::UAV);
        assert_eq!((p.params[2].range_start, p.params[2].range_count), (0, 2));
        assert_eq!(p.ranges[1].num_descriptors, root::UNBOUNDED);
        assert_eq!(p.ranges[0].offset_in_table, root::APPEND);
        let bytes = p.encode().unwrap();
        assert_eq!(RootSigDesc::decode(&bytes).unwrap(), p);
    }

    #[test]
    fn unsupported_visibilities_are_not_silently_widened() {
        assert_eq!(visibility(2), Err(Hr::E_NOTIMPL));
        assert_eq!(visibility(5), Ok(ShaderVisibility::FRAGMENT));
        assert_eq!(visibility(9), Err(Hr::E_INVALIDARG));
    }

    fn params(filter: u32) -> SamplerParams {
        SamplerParams {
            filter,
            address_u: 1,
            address_v: 2,
            address_w: 5,
            mip_lod_bias: -0.5,
            max_anisotropy: 16,
            comparison: 4,
            border_color: [0.0, 0.0, 0.0, 1.0],
            min_lod: 1.0,
            max_lod: 9.0,
        }
    }

    /// D3D12 spec (SDK 10.0.22621) D3D12_SAMPLER_DESC / D3D12_FILTER.
    #[test]
    fn sampler_fields_follow_the_spec() {
        // every valid D3D12_FILTER value: point/linear combinations x reduction, anisotropic x reduction
        for red in 0..4u32 {
            for bits in 0..8u32 {
                let f = red << 7 | (bits & 1) | (bits >> 1 & 1) << 2 | (bits >> 2 & 1) << 4;
                let d = sampler(&params(f)).unwrap();
                let lin = |b: u32| {
                    if bits >> b & 1 == 1 {
                        Filter::LINEAR
                    } else {
                        Filter::NEAREST
                    }
                };
                assert_eq!(
                    (d.mip_filter, d.mag_filter, d.min_filter),
                    (lin(0), lin(1), lin(2)),
                    "{f:#x}"
                );
                assert_eq!(
                    d.max_anisotropy, 1,
                    "MaxAnisotropy is ignored without the anisotropic filter {f:#x}"
                );
                assert_eq!(d.reduction.0 as u32, red);
                // the comparison function counts for comparison filters only
                assert_eq!(d.compare == CompareFunc::LESS_EQUAL, red == 1);
            }
            let f = 0x55 | red << 7; // ANISOTROPIC, COMPARISON_ANISOTROPIC, MINIMUM_/MAXIMUM_ANISOTROPIC
            let d = sampler(&params(f)).unwrap();
            assert_eq!(
                (d.mip_filter, d.mag_filter, d.min_filter),
                (Filter::LINEAR, Filter::LINEAR, Filter::LINEAR)
            );
            assert_eq!(d.max_anisotropy, 16);
        }
        // anisotropy outside 1..=16 is clamped for anisotropic filters
        let mut p = params(0x55);
        p.max_anisotropy = 0;
        assert_eq!(sampler(&p).unwrap().max_anisotropy, 1);
        p.max_anisotropy = 99;
        assert_eq!(sampler(&p).unwrap().max_anisotropy, 16);
        // addressing: WRAP, MIRROR, CLAMP, BORDER, MIRROR_ONCE are 1..=5 in D3D12
        let d = sampler(&params(0x15)).unwrap();
        assert_eq!(
            (d.address_u, d.address_v, d.address_w),
            (
                AddressMode::WRAP,
                AddressMode::MIRROR,
                AddressMode::MIRROR_ONCE
            )
        );
        for m in 1..=5u32 {
            assert_eq!(address_mode(m).unwrap().0 as u32, m - 1);
        }
        assert!(address_mode(0).is_err() && address_mode(6).is_err());
        // LOD range, bias and border colour pass through
        assert_eq!(
            (d.min_lod, d.max_lod, d.mip_lod_bias, d.border_color),
            (1.0, 9.0, -0.5, [0.0, 0.0, 0.0, 1.0])
        );
        // reserved bits are refused; a comparison filter needs a function
        assert!(sampler(&params(0x2)).is_err());
        let mut c = params(0x80 | 0x15);
        c.comparison = 0;
        assert!(sampler(&c).is_err());
    }
}
