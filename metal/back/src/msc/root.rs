//! A root signature as this backend sees it: plain Rust types the proto
//! interpreter fills from the wire form (`RootSigDesc`), and the conversion
//! to the C descriptor Metal Shader Converter takes.
//!
//! The layout of the top-level argument buffer (TLAB) is a function of the
//! root signature alone: `IRRootSignatureGetResourceLocations` returns, for
//! each root parameter in order, its byte offset and size, with the
//! alignment padding MSC inserts (8-byte alignment for pointers, 4 for
//! 32-bit constants; see the user manual, "Explicit layout via Root
//! Signatures"). Never compute offsets as "index times size".

use super::ffi;

/// Which shader stages see a root parameter or static sampler (the numeric
/// values of `IRShaderVisibility`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    All = 0,
    Vertex = 1,
    Hull = 2,
    Domain = 3,
    Geometry = 4,
    Pixel = 5,
    Amplification = 6,
    Mesh = 7,
}

/// A descriptor range's kind (`IRDescriptorRangeType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeKind {
    Srv = 0,
    Uav = 1,
    Cbv = 2,
    Sampler = 3,
}

/// `OffsetInDescriptorsFromTableStart` meaning "after the previous range".
pub const OFFSET_APPEND: u32 = 0xFFFF_FFFF;
/// `NumDescriptors` meaning "unbounded".
pub const UNBOUNDED: u32 = 0xFFFF_FFFF;

/// One range of a descriptor table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Range {
    pub kind: RangeKind,
    /// [`UNBOUNDED`] for an open-ended range.
    pub count: u32,
    pub base_register: u32,
    pub space: u32,
    /// [`OFFSET_APPEND`] or the slot the range starts at.
    pub offset: u32,
    /// `IRDescriptorRangeFlags` bits.
    pub flags: u32,
}

/// What a root parameter is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParamKind {
    /// Inline 32-bit constants (`num_32bit` dwords).
    Constants {
        register: u32,
        space: u32,
        num_32bit: u32,
    },
    /// A root constant buffer: a 64-bit GPU address in the TLAB.
    Cbv { register: u32, space: u32 },
    /// A root SRV (raw buffer): a 64-bit GPU address.
    Srv { register: u32, space: u32 },
    /// A root UAV (raw buffer): a 64-bit GPU address.
    Uav { register: u32, space: u32 },
    /// A descriptor table: a 64-bit GPU address of the first entry.
    Table(Vec<Range>),
}

/// One root parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub kind: ParamKind,
    pub visibility: Visibility,
}

/// A static sampler (`IRStaticSamplerDescriptor`; the numeric fields are
/// the D3D filter / address / comparison / border values the IR enums
/// share).
#[derive(Clone, Debug, PartialEq)]
pub struct StaticSampler {
    pub filter: u32,
    pub address: [u32; 3],
    pub mip_lod_bias: f32,
    pub max_anisotropy: u32,
    pub comparison: u32,
    pub border: u32,
    pub min_lod: f32,
    pub max_lod: f32,
    pub register: u32,
    pub space: u32,
    pub visibility: Visibility,
}

/// Root signature flag bits (this crate's own; mapped to `IRRootSignatureFlags`).
pub mod flags {
    /// Shaders index the CBV/SRV/UAV heap directly (`ResourceDescriptorHeap[i]`).
    pub const HEAP_INDEXED: u32 = 1 << 0;
    /// Shaders index the sampler heap directly (`SamplerDescriptorHeap[i]`).
    pub const SAMPLER_HEAP_INDEXED: u32 = 1 << 1;
    /// Vertex input comes from an input layout (graphics only).
    pub const ALLOW_INPUT_LAYOUT: u32 = 1 << 2;
    pub const ALL_BITS: u32 = 0b111;
}

/// A whole root signature.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RootSignatureDesc {
    pub flags: u32,
    pub params: Vec<Param>,
    pub static_samplers: Vec<StaticSampler>,
}

/// What a top-level argument buffer entry holds (`IRResourceType`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgKind {
    /// A pointer to a descriptor table.
    Table,
    /// Inline 32-bit constants.
    Constants,
    Cbv,
    Srv,
    Uav,
    Sampler,
    Invalid,
}

impl ArgKind {
    pub(crate) fn from_ir(v: u32) -> ArgKind {
        match v {
            ffi::resource_type::TABLE => ArgKind::Table,
            ffi::resource_type::CONSTANT => ArgKind::Constants,
            ffi::resource_type::CBV => ArgKind::Cbv,
            ffi::resource_type::SRV => ArgKind::Srv,
            ffi::resource_type::UAV => ArgKind::Uav,
            ffi::resource_type::SAMPLER => ArgKind::Sampler,
            _ => ArgKind::Invalid,
        }
    }
}

/// Where one entry lives in the top-level argument buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArgLocation {
    pub kind: ArgKind,
    /// Register space (`u32::MAX` for a table, which has none).
    pub space: u32,
    /// Shader register (`u32::MAX` for a table).
    pub slot: u32,
    /// Byte offset into the top-level argument buffer.
    pub offset: u32,
    /// Size in bytes.
    pub size: u64,
}

/// The layout of the top-level argument buffer of a root signature.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TopLevelLayout {
    /// One entry per root parameter, in parameter order.
    pub entries: Vec<ArgLocation>,
    /// Bytes the buffer needs (end of the last entry, rounded up to 8).
    pub size: u32,
}

impl TopLevelLayout {
    pub(crate) fn from_locations(entries: Vec<ArgLocation>) -> TopLevelLayout {
        let end = entries
            .iter()
            .map(|e| u64::from(e.offset) + e.size)
            .max()
            .unwrap_or(0);
        TopLevelLayout {
            size: end.next_multiple_of(8) as u32,
            entries,
        }
    }
}

/// The C form of a [`RootSignatureDesc`]: the arrays the descriptor points
/// into, kept alive together.
pub(crate) struct CRootSignature {
    // The descriptor's pointers point into these vectors' heap buffers,
    // which do not move when the struct does.
    _ranges: Vec<Vec<ffi::IRDescriptorRange1>>,
    _params: Vec<ffi::IRRootParameter1>,
    _samplers: Vec<ffi::IRStaticSamplerDescriptor>,
    pub desc: ffi::IRVersionedRootSignatureDescriptor,
}

impl CRootSignature {
    pub(crate) fn new(d: &RootSignatureDesc) -> CRootSignature {
        let mut all_ranges: Vec<Vec<ffi::IRDescriptorRange1>> = Vec::new();
        for p in &d.params {
            if let ParamKind::Table(ranges) = &p.kind {
                all_ranges.push(
                    ranges
                        .iter()
                        .map(|r| ffi::IRDescriptorRange1 {
                            RangeType: r.kind as u32,
                            NumDescriptors: r.count,
                            BaseShaderRegister: r.base_register,
                            RegisterSpace: r.space,
                            Flags: r.flags,
                            OffsetInDescriptorsFromTableStart: r.offset,
                        })
                        .collect(),
                );
            }
        }
        let mut next_table = 0;
        let params: Vec<ffi::IRRootParameter1> = d
            .params
            .iter()
            .map(|p| {
                let (ty, data) = match &p.kind {
                    ParamKind::Constants {
                        register,
                        space,
                        num_32bit,
                    } => (
                        ffi::param_type::CONSTANTS,
                        ffi::IRRootParameter1Data {
                            Constants: ffi::IRRootConstants {
                                ShaderRegister: *register,
                                RegisterSpace: *space,
                                Num32BitValues: *num_32bit,
                            },
                        },
                    ),
                    ParamKind::Cbv { register, space }
                    | ParamKind::Srv { register, space }
                    | ParamKind::Uav { register, space } => (
                        match p.kind {
                            ParamKind::Cbv { .. } => ffi::param_type::CBV,
                            ParamKind::Srv { .. } => ffi::param_type::SRV,
                            _ => ffi::param_type::UAV,
                        },
                        ffi::IRRootParameter1Data {
                            Descriptor: ffi::IRRootDescriptor1 {
                                ShaderRegister: *register,
                                RegisterSpace: *space,
                                // Not `DataVolatile`/static: the contents are the
                                // application's; MSC ignores these for layout.
                                Flags: 0,
                            },
                        },
                    ),
                    ParamKind::Table(ranges) => {
                        let table = &all_ranges[next_table];
                        next_table += 1;
                        (
                            ffi::param_type::TABLE,
                            ffi::IRRootParameter1Data {
                                DescriptorTable: ffi::IRRootDescriptorTable1 {
                                    NumDescriptorRanges: ranges.len() as u32,
                                    pDescriptorRanges: table.as_ptr(),
                                },
                            },
                        )
                    }
                };
                ffi::IRRootParameter1 {
                    ParameterType: ty,
                    data,
                    ShaderVisibility: p.visibility as u32,
                }
            })
            .collect();
        let samplers: Vec<ffi::IRStaticSamplerDescriptor> = d
            .static_samplers
            .iter()
            .map(|s| ffi::IRStaticSamplerDescriptor {
                Filter: s.filter,
                AddressU: s.address[0],
                AddressV: s.address[1],
                AddressW: s.address[2],
                MipLODBias: s.mip_lod_bias,
                MaxAnisotropy: s.max_anisotropy,
                ComparisonFunc: s.comparison,
                BorderColor: s.border,
                MinLOD: s.min_lod,
                MaxLOD: s.max_lod,
                ShaderRegister: s.register,
                RegisterSpace: s.space,
                ShaderVisibility: s.visibility as u32,
            })
            .collect();
        // `IRRootSignatureFlags`: heap-indexed 0x400, sampler heap 0x800,
        // allow input layout 0x1.
        let mut f = 0;
        if d.flags & flags::HEAP_INDEXED != 0 {
            f |= 0x400;
        }
        if d.flags & flags::SAMPLER_HEAP_INDEXED != 0 {
            f |= 0x800;
        }
        if d.flags & flags::ALLOW_INPUT_LAYOUT != 0 {
            f |= 0x1;
        }
        let desc = ffi::IRVersionedRootSignatureDescriptor {
            version: ffi::ROOT_SIGNATURE_VERSION_1_1,
            desc_1_1: ffi::IRRootSignatureDescriptor1 {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                NumStaticSamplers: samplers.len() as u32,
                pStaticSamplers: samplers.as_ptr(),
                Flags: f,
            },
        };
        CRootSignature {
            _ranges: all_ranges,
            _params: params,
            _samplers: samplers,
            desc,
        }
    }
}
