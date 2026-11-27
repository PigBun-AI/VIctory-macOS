//! Root-signature (de)serialization at the C ABI: `D3D12_ROOT_SIGNATURE_DESC`
//! ↔ the core's `RootSignatureDesc`, blobs, and the deserializer objects.

use std::ffi::c_void;

use windows::Win32::Graphics::Direct3D::{ID3DBlob, ID3DBlob_Impl};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{ComObject, Result as WResult, implement};

use super::private::PrivateData;
use super::{werr, wres};
use crate::hresult::Hr;
use crate::rootsig::{self, Param, ParamKind, Range, RootSignatureDesc, StaticSampler, Version};

#[implement(ID3DBlob)]
pub struct BlobObj {
    pub bytes: Vec<u8>,
}

impl ID3DBlob_Impl for BlobObj_Impl {
    fn GetBufferPointer(&self) -> *mut c_void {
        self.bytes.as_ptr() as *mut c_void
    }
    fn GetBufferSize(&self) -> usize {
        self.bytes.len()
    }
}

pub fn blob(bytes: Vec<u8>) -> ID3DBlob {
    ComObject::new(BlobObj { bytes }).to_interface()
}

fn slice<'a, T>(p: *const T, n: u32) -> WResult<&'a [T]> {
    if n == 0 {
        return Ok(&[]);
    }
    if p.is_null() {
        return Err(werr(Hr::E_INVALIDARG));
    }
    // SAFETY: `n` elements at `p`, the caller's array.
    Ok(unsafe { std::slice::from_raw_parts(p, n as usize) })
}

fn sampler(s: &D3D12_STATIC_SAMPLER_DESC) -> StaticSampler {
    StaticSampler {
        filter: s.Filter.0 as u32,
        address_u: s.AddressU.0 as u32,
        address_v: s.AddressV.0 as u32,
        address_w: s.AddressW.0 as u32,
        mip_lod_bias: s.MipLODBias,
        max_anisotropy: s.MaxAnisotropy,
        comparison: s.ComparisonFunc.0 as u32,
        border: s.BorderColor.0 as u32,
        min_lod: s.MinLOD,
        max_lod: s.MaxLOD,
        reg: s.ShaderRegister,
        space: s.RegisterSpace,
        visibility: s.ShaderVisibility.0 as u32,
    }
}

fn c_sampler(s: &StaticSampler) -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER(s.filter as i32),
        AddressU: D3D12_TEXTURE_ADDRESS_MODE(s.address_u as i32),
        AddressV: D3D12_TEXTURE_ADDRESS_MODE(s.address_v as i32),
        AddressW: D3D12_TEXTURE_ADDRESS_MODE(s.address_w as i32),
        MipLODBias: s.mip_lod_bias,
        MaxAnisotropy: s.max_anisotropy,
        ComparisonFunc: D3D12_COMPARISON_FUNC(s.comparison as i32),
        BorderColor: D3D12_STATIC_BORDER_COLOR(s.border as i32),
        MinLOD: s.min_lod,
        MaxLOD: s.max_lod,
        ShaderRegister: s.reg,
        RegisterSpace: s.space,
        ShaderVisibility: D3D12_SHADER_VISIBILITY(s.visibility as i32),
    }
}

/// Read a version 1.0 description.
///
/// # Safety
/// `d` is a valid `D3D12_ROOT_SIGNATURE_DESC` with valid arrays.
pub unsafe fn read_1_0(d: &D3D12_ROOT_SIGNATURE_DESC) -> WResult<RootSignatureDesc> {
    let mut params = Vec::new();
    for p in slice(d.pParameters, d.NumParameters)? {
        let kind = match p.ParameterType.0 as u32 {
            rootsig::PARAM_TABLE => {
                // SAFETY: `ParameterType` names the live union member.
                let t = unsafe { p.Anonymous.DescriptorTable };
                let ranges = slice(t.pDescriptorRanges, t.NumDescriptorRanges)?
                    .iter()
                    .map(|r| Range {
                        ty: r.RangeType.0 as u32,
                        num: r.NumDescriptors,
                        base: r.BaseShaderRegister,
                        space: r.RegisterSpace,
                        flags: 0,
                        offset: r.OffsetInDescriptorsFromTableStart,
                    })
                    .collect();
                ParamKind::Table(ranges)
            }
            rootsig::PARAM_CONSTANTS => {
                // SAFETY: as above.
                let c = unsafe { p.Anonymous.Constants };
                ParamKind::Constants {
                    reg: c.ShaderRegister,
                    space: c.RegisterSpace,
                    num: c.Num32BitValues,
                }
            }
            ty @ (rootsig::PARAM_CBV | rootsig::PARAM_SRV | rootsig::PARAM_UAV) => {
                // SAFETY: as above.
                let r = unsafe { p.Anonymous.Descriptor };
                ParamKind::Descriptor {
                    ty,
                    reg: r.ShaderRegister,
                    space: r.RegisterSpace,
                    flags: 0,
                }
            }
            _ => return Err(werr(Hr::E_INVALIDARG)),
        };
        params.push(Param {
            kind,
            visibility: p.ShaderVisibility.0 as u32,
        });
    }
    Ok(RootSignatureDesc {
        version: Version::V1_0,
        params,
        samplers: slice(d.pStaticSamplers, d.NumStaticSamplers)?
            .iter()
            .map(sampler)
            .collect(),
        flags: d.Flags.0 as u32,
    })
}

/// Read a version 1.1 description.
///
/// # Safety
/// As [`read_1_0`].
pub unsafe fn read_1_1(d: &D3D12_ROOT_SIGNATURE_DESC1) -> WResult<RootSignatureDesc> {
    let mut params = Vec::new();
    for p in slice(d.pParameters, d.NumParameters)? {
        let kind = match p.ParameterType.0 as u32 {
            rootsig::PARAM_TABLE => {
                // SAFETY: `ParameterType` names the live union member.
                let t = unsafe { p.Anonymous.DescriptorTable };
                let ranges = slice(t.pDescriptorRanges, t.NumDescriptorRanges)?
                    .iter()
                    .map(|r| Range {
                        ty: r.RangeType.0 as u32,
                        num: r.NumDescriptors,
                        base: r.BaseShaderRegister,
                        space: r.RegisterSpace,
                        flags: r.Flags.0 as u32,
                        offset: r.OffsetInDescriptorsFromTableStart,
                    })
                    .collect();
                ParamKind::Table(ranges)
            }
            rootsig::PARAM_CONSTANTS => {
                // SAFETY: as above.
                let c = unsafe { p.Anonymous.Constants };
                ParamKind::Constants {
                    reg: c.ShaderRegister,
                    space: c.RegisterSpace,
                    num: c.Num32BitValues,
                }
            }
            ty @ (rootsig::PARAM_CBV | rootsig::PARAM_SRV | rootsig::PARAM_UAV) => {
                // SAFETY: as above.
                let r = unsafe { p.Anonymous.Descriptor };
                ParamKind::Descriptor {
                    ty,
                    reg: r.ShaderRegister,
                    space: r.RegisterSpace,
                    flags: r.Flags.0 as u32,
                }
            }
            _ => return Err(werr(Hr::E_INVALIDARG)),
        };
        params.push(Param {
            kind,
            visibility: p.ShaderVisibility.0 as u32,
        });
    }
    Ok(RootSignatureDesc {
        version: Version::V1_1,
        params,
        samplers: slice(d.pStaticSamplers, d.NumStaticSamplers)?
            .iter()
            .map(sampler)
            .collect(),
        flags: d.Flags.0 as u32,
    })
}

/// A description in C layout, with the arrays it points to.
pub struct Desc10 {
    _params: Vec<D3D12_ROOT_PARAMETER>,
    _ranges: Vec<Vec<D3D12_DESCRIPTOR_RANGE>>,
    _samplers: Vec<D3D12_STATIC_SAMPLER_DESC>,
    pub desc: D3D12_ROOT_SIGNATURE_DESC,
}

pub struct Desc11 {
    _params: Vec<D3D12_ROOT_PARAMETER1>,
    _ranges: Vec<Vec<D3D12_DESCRIPTOR_RANGE1>>,
    _samplers: Vec<D3D12_STATIC_SAMPLER_DESC>,
    pub desc: D3D12_ROOT_SIGNATURE_DESC1,
}

// SAFETY: the raw pointers point into the owned vectors, which never change.
unsafe impl Send for Desc10 {}
unsafe impl Sync for Desc10 {}
unsafe impl Send for Desc11 {}
unsafe impl Sync for Desc11 {}

pub fn write_1_0(d: &RootSignatureDesc) -> Box<Desc10> {
    let d = d.to_version(Version::V1_0);
    let mut ranges: Vec<Vec<D3D12_DESCRIPTOR_RANGE>> = Vec::new();
    let mut params = Vec::new();
    for p in &d.params {
        let mut cp = D3D12_ROOT_PARAMETER {
            ShaderVisibility: D3D12_SHADER_VISIBILITY(p.visibility as i32),
            ..Default::default()
        };
        match &p.kind {
            ParamKind::Table(rs) => {
                let v: Vec<_> = rs
                    .iter()
                    .map(|r| D3D12_DESCRIPTOR_RANGE {
                        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE(r.ty as i32),
                        NumDescriptors: r.num,
                        BaseShaderRegister: r.base,
                        RegisterSpace: r.space,
                        OffsetInDescriptorsFromTableStart: r.offset,
                    })
                    .collect();
                cp.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
                cp.Anonymous.DescriptorTable = D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: v.len() as u32,
                    pDescriptorRanges: v.as_ptr(),
                };
                ranges.push(v);
            }
            ParamKind::Constants { reg, space, num } => {
                cp.ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
                cp.Anonymous.Constants = D3D12_ROOT_CONSTANTS {
                    ShaderRegister: *reg,
                    RegisterSpace: *space,
                    Num32BitValues: *num,
                };
            }
            ParamKind::Descriptor { ty, reg, space, .. } => {
                cp.ParameterType = D3D12_ROOT_PARAMETER_TYPE(*ty as i32);
                cp.Anonymous.Descriptor = D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: *reg,
                    RegisterSpace: *space,
                };
            }
        }
        params.push(cp);
    }
    let samplers: Vec<_> = d.samplers.iter().map(c_sampler).collect();
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: samplers.len() as u32,
        pStaticSamplers: samplers.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAGS(d.flags as i32),
    };
    Box::new(Desc10 {
        _params: params,
        _ranges: ranges,
        _samplers: samplers,
        desc,
    })
}

pub fn write_1_1(d: &RootSignatureDesc) -> Box<Desc11> {
    let d = d.to_version(Version::V1_1);
    let mut ranges: Vec<Vec<D3D12_DESCRIPTOR_RANGE1>> = Vec::new();
    let mut params = Vec::new();
    for p in &d.params {
        let mut cp = D3D12_ROOT_PARAMETER1 {
            ShaderVisibility: D3D12_SHADER_VISIBILITY(p.visibility as i32),
            ..Default::default()
        };
        match &p.kind {
            ParamKind::Table(rs) => {
                let v: Vec<_> = rs
                    .iter()
                    .map(|r| D3D12_DESCRIPTOR_RANGE1 {
                        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE(r.ty as i32),
                        NumDescriptors: r.num,
                        BaseShaderRegister: r.base,
                        RegisterSpace: r.space,
                        Flags: D3D12_DESCRIPTOR_RANGE_FLAGS(r.flags as i32),
                        OffsetInDescriptorsFromTableStart: r.offset,
                    })
                    .collect();
                cp.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
                cp.Anonymous.DescriptorTable = D3D12_ROOT_DESCRIPTOR_TABLE1 {
                    NumDescriptorRanges: v.len() as u32,
                    pDescriptorRanges: v.as_ptr(),
                };
                ranges.push(v);
            }
            ParamKind::Constants { reg, space, num } => {
                cp.ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
                cp.Anonymous.Constants = D3D12_ROOT_CONSTANTS {
                    ShaderRegister: *reg,
                    RegisterSpace: *space,
                    Num32BitValues: *num,
                };
            }
            ParamKind::Descriptor {
                ty,
                reg,
                space,
                flags,
            } => {
                cp.ParameterType = D3D12_ROOT_PARAMETER_TYPE(*ty as i32);
                cp.Anonymous.Descriptor = D3D12_ROOT_DESCRIPTOR1 {
                    ShaderRegister: *reg,
                    RegisterSpace: *space,
                    Flags: D3D12_ROOT_DESCRIPTOR_FLAGS(*flags as i32),
                };
            }
        }
        params.push(cp);
    }
    let samplers: Vec<_> = d.samplers.iter().map(c_sampler).collect();
    let desc = D3D12_ROOT_SIGNATURE_DESC1 {
        NumParameters: params.len() as u32,
        pParameters: params.as_ptr(),
        NumStaticSamplers: samplers.len() as u32,
        pStaticSamplers: samplers.as_ptr(),
        Flags: D3D12_ROOT_SIGNATURE_FLAGS(d.flags as i32),
    };
    Box::new(Desc11 {
        _params: params,
        _ranges: ranges,
        _samplers: samplers,
        desc,
    })
}

/// `D3D12SerializeRootSignature`: a 1.0 description; 1.1 here is `E_INVALIDARG`, as in D3DMetal.
///
/// # Safety
/// `d` is a valid description.
pub unsafe fn serialize_1_0(
    d: &D3D12_ROOT_SIGNATURE_DESC,
    version: D3D_ROOT_SIGNATURE_VERSION,
) -> WResult<ID3DBlob> {
    if version != D3D_ROOT_SIGNATURE_VERSION_1_0 {
        return Err(werr(Hr::E_INVALIDARG));
    }
    // SAFETY: forwarded contract.
    let desc = unsafe { read_1_0(d) }?;
    Ok(blob(wres(rootsig::serialize(&desc))?))
}

/// `D3D12SerializeVersionedRootSignature`.
///
/// # Safety
/// `d` is a valid versioned description.
pub unsafe fn serialize_versioned(d: &D3D12_VERSIONED_ROOT_SIGNATURE_DESC) -> WResult<ID3DBlob> {
    // SAFETY: `Version` names the live union member.
    let desc = unsafe {
        match d.Version {
            D3D_ROOT_SIGNATURE_VERSION_1_0 => read_1_0(&d.Anonymous.Desc_1_0)?,
            D3D_ROOT_SIGNATURE_VERSION_1_1 => read_1_1(&d.Anonymous.Desc_1_1)?,
            _ => return Err(werr(Hr::E_INVALIDARG)),
        }
    };
    Ok(blob(wres(rootsig::serialize(&desc))?))
}

#[implement(ID3D12RootSignatureDeserializer)]
pub struct RootSigDeserializerObj {
    pub private: PrivateData,
    pub desc: Box<Desc10>,
}

impl RootSigDeserializerObj_Impl {
    pub(super) fn x_GetRootSignatureDesc(&self) -> *mut D3D12_ROOT_SIGNATURE_DESC {
        &self.desc.desc as *const _ as *mut _
    }
}

pub struct Versioned {
    v10: D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
    v11: D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
    _d10: Box<Desc10>,
    _d11: Box<Desc11>,
    original_is_1_1: bool,
}

// SAFETY: as for `Desc10`.
unsafe impl Send for Versioned {}
unsafe impl Sync for Versioned {}

impl Versioned {
    pub fn new(d: &RootSignatureDesc) -> Versioned {
        let d10 = write_1_0(d);
        let d11 = write_1_1(d);
        let mut v10 = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
            Version: D3D_ROOT_SIGNATURE_VERSION_1_0,
            ..Default::default()
        };
        v10.Anonymous.Desc_1_0 = d10.desc;
        let mut v11 = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
            Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
            ..Default::default()
        };
        v11.Anonymous.Desc_1_1 = d11.desc;
        Versioned {
            v10,
            v11,
            _d10: d10,
            _d11: d11,
            original_is_1_1: d.version == Version::V1_1,
        }
    }
}

#[implement(ID3D12VersionedRootSignatureDeserializer)]
pub struct VersionedDeserializerObj {
    pub private: PrivateData,
    pub desc: Versioned,
}

impl VersionedDeserializerObj_Impl {
    pub(super) fn x_GetRootSignatureDescAtVersion(
        &self,
        converttoversion: D3D_ROOT_SIGNATURE_VERSION,
    ) -> WResult<*mut D3D12_VERSIONED_ROOT_SIGNATURE_DESC> {
        match converttoversion {
            D3D_ROOT_SIGNATURE_VERSION_1_0 => Ok(&self.desc.v10 as *const _ as *mut _),
            D3D_ROOT_SIGNATURE_VERSION_1_1 => Ok(&self.desc.v11 as *const _ as *mut _),
            _ => Err(werr(Hr::E_INVALIDARG)),
        }
    }

    pub(super) fn x_GetUnconvertedRootSignatureDesc(
        &self,
    ) -> *mut D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        if self.desc.original_is_1_1 {
            &self.desc.v11 as *const _ as *mut _
        } else {
            &self.desc.v10 as *const _ as *mut _
        }
    }
}
