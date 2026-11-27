//! Proto root signatures and DXIL pipelines meet Metal Shader Converter here.
//!
//! * [`to_msc`]: a proto [`RootSigDesc`] as the converter's own description.
//! * [`layout_from_msc`]: MSC's reflection of that description as a
//!   [`TlabLayout`] (MSC's alignment padding and the static-sampler pointer
//!   it appends to the argument buffer).
//! * [`status_of_msc`]: what each way of failing answers.
//!
//! The converter is loaded on first use (`dlopen`, see [`crate::msc`]); a
//! machine without it gets `UNSUPPORTED` from `CREATE_COMPUTE_PIPELINE` and
//! the rule-based layout for root signatures, never an abort.

use victory_metal_proto::root::{RootSigDesc, root_flags};
use victory_metal_proto::status::Status;
use victory_metal_proto::types::{
    AddressMode, CompareFunc, Filter, RangeKind, Reduction, RootParamKind, ShaderVisibility,
};

use std::sync::Arc;

use victory_metal_proto::dropstats::{Label, PipeType};
use victory_metal_proto::ids::PsoId;
use victory_metal_proto::sync::CreateComputePipelineArgs;

use super::sync::{exhausted, range_is_mapped};
use super::tlab::{LayoutError, TlabLayout};
use super::{Executor, Pso, PsoBody, status_of};
use crate::msc::{self, CompileOptions, MscError};
use crate::sampler::SamplerSpec;

/// Largest DXIL container `CREATE_COMPUTE_PIPELINE` takes (the biggest shader
/// of the VIctory cache is 1.4 MB).
const MAX_DXIL_BYTES: u64 = 64 << 20;

fn visibility(v: ShaderVisibility) -> msc::Visibility {
    match v {
        ShaderVisibility::VERTEX => msc::Visibility::Vertex,
        ShaderVisibility::FRAGMENT => msc::Visibility::Pixel,
        ShaderVisibility::OBJECT => msc::Visibility::Amplification,
        ShaderVisibility::MESH => msc::Visibility::Mesh,
        _ => msc::Visibility::All,
    }
}

fn range_kind(k: RangeKind) -> msc::RangeKind {
    match k {
        RangeKind::UAV => msc::RangeKind::Uav,
        RangeKind::CBV => msc::RangeKind::Cbv,
        RangeKind::SAMPLER => msc::RangeKind::Sampler,
        _ => msc::RangeKind::Srv,
    }
}

/// The filter the converter takes: the original API's bit encoding (mip bit 0,
/// mag bit 2, min bit 4, anisotropic 0x40, reduction in bits 7-8).
fn filter_bits(s: &victory_metal_proto::types::SamplerDesc) -> u32 {
    let reduction = match s.reduction {
        Reduction::COMPARISON => 1,
        Reduction::MIN => 2,
        Reduction::MAX => 3,
        _ => 0,
    } << 7;
    if s.max_anisotropy > 1 {
        return reduction | 0x55;
    }
    let bit = |f: Filter, shift: u32| u32::from(f == Filter::LINEAR) << shift;
    reduction | bit(s.min_filter, 4) | bit(s.mag_filter, 2) | bit(s.mip_filter, 0)
}

fn address(a: AddressMode) -> u32 {
    match a {
        AddressMode::MIRROR => 2,
        AddressMode::CLAMP => 3,
        AddressMode::BORDER => 4,
        AddressMode::MIRROR_ONCE => 5,
        _ => 1,
    }
}

fn compare(c: CompareFunc) -> u32 {
    (c.0 as u32 + 1).min(8)
}

/// The static border colors are three: transparent black, opaque black, opaque white.
fn border(c: [f32; 4]) -> u32 {
    match c {
        [r, g, b, a] if r == 1.0 && g == 1.0 && b == 1.0 && a == 1.0 => 2,
        [r, g, b, a] if r == 0.0 && g == 0.0 && b == 0.0 && a == 1.0 => 1,
        _ => 0,
    }
}

/// Converts a decoded proto root signature to the converter's description.
/// The description was checked by `RootSigDesc::decode` (tables stay inside
/// the range array).
pub(crate) fn to_msc(desc: &RootSigDesc) -> msc::RootSignatureDesc {
    let mut flags = 0;
    if desc.flags & root_flags::CBV_SRV_UAV_HEAP_INDEXED != 0 {
        flags |= msc::flags::HEAP_INDEXED;
    }
    if desc.flags & root_flags::SAMPLER_HEAP_INDEXED != 0 {
        flags |= msc::flags::SAMPLER_HEAP_INDEXED;
    }
    let params = desc
        .params
        .iter()
        .map(|p| {
            let kind = match p.kind {
                RootParamKind::CONSTANTS => msc::ParamKind::Constants {
                    register: p.shader_register,
                    space: p.register_space,
                    num_32bit: p.num_32bit_values,
                },
                RootParamKind::CBV => msc::ParamKind::Cbv {
                    register: p.shader_register,
                    space: p.register_space,
                },
                RootParamKind::SRV => msc::ParamKind::Srv {
                    register: p.shader_register,
                    space: p.register_space,
                },
                RootParamKind::UAV => msc::ParamKind::Uav {
                    register: p.shader_register,
                    space: p.register_space,
                },
                _ => {
                    let start = p.range_start as usize;
                    let end = start + p.range_count as usize;
                    msc::ParamKind::Table(
                        desc.ranges
                            .get(start..end)
                            .unwrap_or(&[])
                            .iter()
                            .map(|r| msc::Range {
                                kind: range_kind(r.kind),
                                count: r.num_descriptors,
                                base_register: r.base_register,
                                space: r.register_space,
                                offset: r.offset_in_table,
                                flags: r.flags,
                            })
                            .collect(),
                    )
                }
            };
            msc::Param {
                kind,
                visibility: visibility(p.visibility),
            }
        })
        .collect();
    let static_samplers = desc
        .static_samplers
        .iter()
        .map(|s| msc::StaticSampler {
            filter: filter_bits(&s.sampler),
            address: [
                address(s.sampler.address_u),
                address(s.sampler.address_v),
                address(s.sampler.address_w),
            ],
            mip_lod_bias: s.sampler.mip_lod_bias,
            max_anisotropy: s.sampler.max_anisotropy,
            comparison: compare(s.sampler.compare),
            border: border(s.sampler.border_color),
            min_lod: s.sampler.min_lod,
            max_lod: s.sampler.max_lod,
            register: s.shader_register,
            space: s.register_space,
            visibility: visibility(s.visibility),
        })
        .collect();
    msc::RootSignatureDesc {
        flags,
        params,
        static_samplers,
    }
}

/// A proto sampler as the core's [`SamplerSpec`]. Min/max reduction modes have no
/// counterpart in the sampler descriptor and read as the standard filter.
pub(crate) fn sampler_spec(s: &victory_metal_proto::types::SamplerDesc) -> SamplerSpec {
    use crate::sampler::{Address, Border, Compare, Filter as F};
    let f = |f: Filter| {
        if f == Filter::LINEAR {
            F::Linear
        } else {
            F::Nearest
        }
    };
    let a = |m: AddressMode| match m {
        AddressMode::MIRROR => Address::MirrorRepeat,
        AddressMode::CLAMP => Address::ClampToEdge,
        AddressMode::BORDER => Address::ClampToBorder,
        AddressMode::MIRROR_ONCE => Address::MirrorClampToEdge,
        _ => Address::Repeat,
    };
    let aniso = s.max_anisotropy > 1;
    let lin = |x: Filter| if aniso { F::Linear } else { f(x) };
    SamplerSpec {
        min: lin(s.min_filter),
        mag: lin(s.mag_filter),
        mip: Some(lin(s.mip_filter)),
        address: [a(s.address_u), a(s.address_v), a(s.address_w)],
        border: match border(s.border_color) {
            2 => Border::OpaqueWhite,
            1 => Border::OpaqueBlack,
            _ => Border::TransparentBlack,
        },
        compare: (s.reduction == Reduction::COMPARISON).then_some(match s.compare {
            CompareFunc::NEVER => Compare::Never,
            CompareFunc::LESS => Compare::Less,
            CompareFunc::EQUAL => Compare::Equal,
            CompareFunc::LESS_EQUAL => Compare::LessEqual,
            CompareFunc::GREATER => Compare::Greater,
            CompareFunc::NOT_EQUAL => Compare::NotEqual,
            CompareFunc::GREATER_EQUAL => Compare::GreaterEqual,
            _ => Compare::Always,
        }),
        min_lod: s.min_lod,
        max_lod: s.max_lod,
        max_anisotropy: s.max_anisotropy.max(1),
        lod_bias: s.mip_lod_bias,
    }
}

/// The layout MSC reflection gives for `desc`, validated against it.
pub(crate) fn layout_from_msc(
    desc: &RootSigDesc,
    layout: &msc::TopLevelLayout,
) -> Result<TlabLayout, LayoutError> {
    let n = desc.params.len();
    let offsets: Vec<u32> = layout.entries.iter().take(n).map(|e| e.offset).collect();
    let static_table = layout
        .static_sampler_table
        .and_then(|i| layout.entries.get(i))
        .map(|e| e.offset);
    TlabLayout::from_reflection_with_static(desc, &offsets, static_table, layout.size)
}

/// The status a failed conversion answers. `last` keeps the full text for
/// diagnostics (`Executor::last_error`).
pub(crate) fn status_of_msc(e: &MscError) -> Status {
    match e {
        // MSC is not installed (or is the wrong build), or it refused a
        // construct it does not convert: this back cannot make the pipeline.
        MscError::Library { .. }
        | MscError::Rejected { .. }
        | MscError::Limit(_)
        | MscError::Unsupported(_) => Status::UNSUPPORTED,
        MscError::RootSignature { .. } | MscError::BadInput(_) => Status::INVALID_ARG,
        MscError::NoOutput(_) | MscError::Mismatch(_) => Status::INTERNAL,
        MscError::Metal(e) => status_of(e),
    }
}

impl Executor {
    /// `CREATE_COMPUTE_PIPELINE`: DXIL + root signature through MSC to a
    /// Metal 4 compute pipeline. The conversion (milliseconds to a second)
    /// runs outside the state lock.
    pub(super) fn create_compute_pipeline(
        &self,
        a: &mut CreateComputePipelineArgs,
    ) -> Result<(), Status> {
        let Some(stats) = crate::dropstats::get() else {
            return self.create_compute_pipeline_inner(a, &mut Label::default());
        };
        crate::dropstats::take_reason();
        let mut label = Label::NONE;
        let r = self.create_compute_pipeline_inner(a, &mut label);
        match &r {
            Ok(()) => {
                stats.register(a.pso.0, label);
                stats.pso_created("compute", label.hash, Ok(a.pso.0));
            }
            Err(s) => {
                let why = crate::dropstats::take_reason().unwrap_or_else(|| format!("{s:?}"));
                stats.pso_created("compute", label.hash, Err(&why));
            }
        }
        r
    }

    fn create_compute_pipeline_inner(
        &self,
        a: &mut CreateComputePipelineArgs,
        label: &mut Label,
    ) -> Result<(), Status> {
        if a.flags != 0 || a.reserved != [0; 4] || a.dxil_addr == 0 || a.dxil_len == 0 {
            return Err(Status::INVALID_ARG);
        }
        if a.dxil_len > MAX_DXIL_BYTES {
            return Err(Status::INVALID_ARG);
        }
        // `msync` wants a page-aligned start: widen the range to whole pages.
        let page = 16384usize;
        let start = a.dxil_addr as usize & !(page - 1);
        let span = a.dxil_addr as usize + a.dxil_len as usize - start;
        if !range_is_mapped(start, span) {
            return Err(Status::INVALID_ARG);
        }
        let (device, sig) = self.with_state(|st| {
            st.root_sigs
                .get(a.root_sig.0)
                .map(|s| (st.device.clone(), s.clone()))
                .ok_or(Status::NOT_FOUND)
        })??;
        // SAFETY: the `Link` contract: the bytes are the caller's, valid for
        // the call (`CreateComputePipelineArgs::dxil_addr`).
        let dxil =
            unsafe { std::slice::from_raw_parts(a.dxil_addr as *const u8, a.dxil_len as usize) };
        if crate::dropstats::get().is_some() {
            *label = crate::dropstats::label_of_stages(PipeType::Cs, &[dxil]);
        }
        let options = CompileOptions {
            compat: self.options.compat,
            ..CompileOptions::default()
        };
        let made = device
            .compute_pipeline_from_dxil(dxil, Some(&sig.msc_desc), &options)
            .map_err(|e| {
                self.note_error(format!("CREATE_COMPUTE_PIPELINE: {e}"));
                status_of_msc(&e)
            })?;
        let threads = made.reflection.threadgroup_size;
        if threads.contains(&0) {
            self.note_error(
                "CREATE_COMPUTE_PIPELINE: the shader reports no thread group size".into(),
            );
            return Err(Status::INTERNAL);
        }
        // The shader reads its arguments where MSC put them for this root
        // signature. They must be where the interpreter writes them.
        if !layout_agrees(&sig, &made.reflection.layout) {
            self.note_error(format!(
                "CREATE_COMPUTE_PIPELINE: the shader's argument layout {:?} is not the root signature's {:?}",
                made.reflection.layout, sig.layout
            ));
            return Err(Status::INTERNAL);
        }
        let pso = Arc::new(Pso {
            body: PsoBody::Compute {
                pipeline: made.pipeline,
                threads,
            },
            root_sig: sig,
        });
        a.pso = PsoId(self.with_state(|st| st.psos.insert(pso).map_err(exhausted))??);
        Ok(())
    }
}

/// Whether the reflected argument buffer is the one the root signature has.
pub(super) fn layout_agrees(sig: &super::RootSig, reflected: &msc::TopLevelLayout) -> bool {
    let n = sig.desc.params.len();
    if reflected.size != sig.layout.size() || reflected.entries.len() < n {
        return false;
    }
    let offsets_agree = (0..n).all(|i| {
        sig.layout
            .slot(i as u32)
            .is_some_and(|s| s.offset == reflected.entries[i].offset)
    });
    let reflected_static = reflected
        .static_sampler_table
        .and_then(|i| reflected.entries.get(i))
        .map(|e| e.offset);
    offsets_agree && reflected_static == sig.layout.static_table_offset()
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_metal_proto::root::{DescriptorRange, RootParam, StaticSampler, UNBOUNDED};
    use victory_metal_proto::types::{RangeKind, SamplerDesc};

    #[test]
    fn filters_use_the_original_bit_encoding() {
        let linear = SamplerDesc {
            min_filter: Filter::LINEAR,
            mag_filter: Filter::LINEAR,
            mip_filter: Filter::LINEAR,
            ..SamplerDesc::default()
        };
        assert_eq!(filter_bits(&linear), 0x15);
        assert_eq!(filter_bits(&SamplerDesc::default()), 0);
        let point_mag = SamplerDesc {
            mag_filter: Filter::LINEAR,
            ..SamplerDesc::default()
        };
        assert_eq!(filter_bits(&point_mag), 0x4);
        let cmp = SamplerDesc {
            min_filter: Filter::LINEAR,
            mag_filter: Filter::LINEAR,
            mip_filter: Filter::LINEAR,
            reduction: Reduction::COMPARISON,
            ..SamplerDesc::default()
        };
        assert_eq!(filter_bits(&cmp), 0x95);
        let aniso = SamplerDesc {
            max_anisotropy: 8,
            ..SamplerDesc::default()
        };
        assert_eq!(filter_bits(&aniso), 0x55);
        assert_eq!(address(AddressMode::WRAP), 1);
        assert_eq!(address(AddressMode::MIRROR_ONCE), 5);
        assert_eq!(compare(CompareFunc::NEVER), 1);
        assert_eq!(compare(CompareFunc::ALWAYS), 8);
        assert_eq!(border([0.0, 0.0, 0.0, 0.0]), 0);
        assert_eq!(border([0.0, 0.0, 0.0, 1.0]), 1);
        assert_eq!(border([1.0; 4]), 2);
    }

    #[test]
    fn a_root_signature_converts_parameter_for_parameter() {
        let desc = RootSigDesc {
            flags: root_flags::CBV_SRV_UAV_HEAP_INDEXED,
            params: vec![
                RootParam {
                    kind: RootParamKind::CONSTANTS,
                    num_32bit_values: 3,
                    shader_register: 1,
                    register_space: 2,
                    ..RootParam::default()
                },
                RootParam {
                    kind: RootParamKind::UAV,
                    shader_register: 4,
                    ..RootParam::default()
                },
                RootParam {
                    kind: RootParamKind::TABLE,
                    range_start: 1,
                    range_count: 1,
                    visibility: ShaderVisibility::FRAGMENT,
                    ..RootParam::default()
                },
            ],
            ranges: vec![
                DescriptorRange::default(),
                DescriptorRange {
                    kind: RangeKind::UAV,
                    num_descriptors: UNBOUNDED,
                    base_register: 7,
                    ..DescriptorRange::default()
                },
            ],
            static_samplers: vec![StaticSampler::default()],
        };
        let m = to_msc(&desc);
        assert_eq!(m.flags, msc::flags::HEAP_INDEXED);
        assert_eq!(m.params.len(), 3);
        assert!(matches!(
            m.params[0].kind,
            msc::ParamKind::Constants {
                register: 1,
                space: 2,
                num_32bit: 3
            }
        ));
        assert!(matches!(
            m.params[1].kind,
            msc::ParamKind::Uav {
                register: 4,
                space: 0
            }
        ));
        assert_eq!(m.params[2].visibility, msc::Visibility::Pixel);
        match &m.params[2].kind {
            msc::ParamKind::Table(r) => {
                assert_eq!(r.len(), 1, "the table takes its own run of ranges");
                assert_eq!(r[0].kind, msc::RangeKind::Uav);
                assert_eq!(r[0].base_register, 7);
                assert_eq!(r[0].count, msc::UNBOUNDED);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(m.static_samplers.len(), 1);
    }
}
