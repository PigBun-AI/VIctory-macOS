//! The root signature the host builds for a translated shader
//! (`victory-gpu` `pipeline/host.rs`, `HostPipeline::new`; also
//! `victory_shader::emit::host`), in the back's own types.
//!
//! Per stage: the user data as root constants `b0` space 0 (when the shader
//! reads any), then its root CBVs (descriptors, space 1), root SRVs and root
//! UAVs (space 1; each a one-view table of the buffer's host copy unless the
//! slot is one of the translator's own, which stay root descriptors), the
//! immediate samplers as static samplers `s<n>` space 0; both heaps
//! indexed, and the input layout flag for graphics stages.

use victory_metal_back::msc::{
    OFFSET_APPEND, Param, ParamKind, Range, RangeKind, RootSignatureDesc, StaticSampler,
    Visibility, flags,
};
use victory_shader::HwStage;
use victory_shader::emit::{
    HostShader, ROOT_CBV_SPACE, SAMPLER_SPACE, SLOT_FAULTS, SLOT_MEMORY, SLOT_STRIDES,
    SLOT_TABLE_STRIDES, SLOT_TABLE_VIEW_ADDRESSES, SLOT_VIEW_ADDRESSES, USER_DATA_SPACE,
};

/// `IRDescriptorRangeFlagDescriptorsVolatile | IRDescriptorRangeFlagDataVolatile`.
const VOLATILE: u32 = 0x1 | 0x2;

/// The visibility of the stage a shader is bound as.
pub fn visibility(stage: HwStage) -> Visibility {
    match stage {
        HwStage::Vs => Visibility::Vertex,
        HwStage::Ps => Visibility::Pixel,
        HwStage::Ms => Visibility::Mesh,
        HwStage::Gs => Visibility::Geometry,
        HwStage::Hs => Visibility::Hull,
        _ => Visibility::All,
    }
}

/// Root descriptors the translator owns (not the title's views): the stride
/// tables, the view address directories, title memory and the fault notes.
fn ours(slot: u8, uav: bool) -> bool {
    if uav {
        slot == SLOT_FAULTS
    } else {
        [
            SLOT_STRIDES,
            SLOT_TABLE_STRIDES,
            SLOT_VIEW_ADDRESSES,
            SLOT_TABLE_VIEW_ADDRESSES,
            SLOT_MEMORY,
        ]
        .contains(&slot)
    }
}

/// The root signature for these stages bound together.
pub fn root_signature(stages: &[(&HostShader, Visibility)], compute: bool) -> RootSignatureDesc {
    let mut params = Vec::new();
    let mut samplers = Vec::new();
    for &(s, visibility) in stages {
        let dwords = s.user_data_dwords();
        if dwords > 0 {
            params.push(Param {
                kind: ParamKind::Constants {
                    register: 0,
                    space: USER_DATA_SPACE,
                    num_32bit: dwords,
                },
                visibility,
            });
        }
        for &slot in &s.root_cbvs {
            params.push(Param {
                kind: ParamKind::Cbv {
                    register: u32::from(slot),
                    space: ROOT_CBV_SPACE,
                },
                visibility,
            });
        }
        for (slots, uav) in [(&s.root_srvs, false), (&s.root_uavs, true)] {
            for &slot in slots {
                let (register, space) = (u32::from(slot), ROOT_CBV_SPACE);
                let kind = if ours(slot, uav) {
                    if uav {
                        ParamKind::Uav { register, space }
                    } else {
                        ParamKind::Srv { register, space }
                    }
                } else {
                    // The host's bounded form: a table of one view.
                    ParamKind::Table(vec![Range {
                        kind: if uav { RangeKind::Uav } else { RangeKind::Srv },
                        count: 1,
                        base_register: register,
                        space,
                        offset: OFFSET_APPEND,
                        flags: VOLATILE,
                    }])
                };
                params.push(Param { kind, visibility });
            }
        }
        for (n, d) in s.static_samplers.iter().enumerate() {
            samplers.push(StaticSampler {
                filter: d.filter_for_host(true),
                address: d.address,
                mip_lod_bias: d.mip_lod_bias,
                max_anisotropy: d.max_anisotropy,
                comparison: d.comparison,
                border: d.border,
                min_lod: d.min_lod,
                max_lod: d.max_lod,
                register: n as u32,
                space: SAMPLER_SPACE,
                visibility,
            });
        }
    }
    let mut f = flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED;
    if !compute {
        f |= flags::ALLOW_INPUT_LAYOUT;
    }
    RootSignatureDesc {
        flags: f,
        params,
        static_samplers: samplers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use victory_shader::RootType;
    use victory_shader::emit::UserData;

    fn shader() -> HostShader {
        HostShader {
            stage: HwStage::Cs,
            dxil: Vec::new(),
            user_data: vec![UserData {
                index: 5,
                slot: 0,
                root: RootType::Constants,
                dword: 5,
            }],
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: vec![1],
            root_srvs: vec![2, SLOT_MEMORY],
            root_uavs: vec![3, SLOT_FAULTS],
            root_indexed_views: Vec::new(),
            vertex_reads: Vec::new(),
        }
    }

    #[test]
    fn user_data_comes_first_and_the_title_views_are_tables() {
        let r = root_signature(&[(&shader(), Visibility::All)], true);
        // 8 dwords (index 5 + 1 rounded up to 4), cbv, srv table, srv root, uav table, uav root.
        assert_eq!(r.params.len(), 6);
        assert!(matches!(
            r.params[0].kind,
            ParamKind::Constants { num_32bit: 8, .. }
        ));
        assert!(matches!(
            r.params[1].kind,
            ParamKind::Cbv {
                register: 1,
                space: 1
            }
        ));
        assert!(matches!(r.params[2].kind, ParamKind::Table(_)));
        assert!(matches!(r.params[3].kind, ParamKind::Srv { .. }));
        assert!(matches!(r.params[4].kind, ParamKind::Table(_)));
        assert!(matches!(r.params[5].kind, ParamKind::Uav { .. }));
        assert_eq!(r.flags, flags::HEAP_INDEXED | flags::SAMPLER_HEAP_INDEXED);
    }

    #[test]
    fn graphics_roots_allow_the_input_layout() {
        let r = root_signature(&[(&shader(), Visibility::Vertex)], false);
        assert_eq!(
            r.flags & flags::ALLOW_INPUT_LAYOUT,
            flags::ALLOW_INPUT_LAYOUT
        );
    }
}
