//! Cube samples with gradients select their level by the footprint (after
//! DexNom/VIctory 921d8d3b).

use super::host::{Dispatch, Host, Root, View};
use super::intersect_tests::build_compute_shader;
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};
use super::host::api::*;

/// The level is the red channel on all six faces so every sample returns
/// its selected mip directly (including the fraction with trilinear filtering).
#[test]
fn guest_cube_gradient_selects_mip_by_footprint() {
    const SIZE: usize = 64;
    const MIPS: u16 = 7;
    let mut words = vec![
        0xBE81_0380,
        0xBE83_0380, // s1, s3 = 0 (upper halves of the tables)
        0xF40C_0400,
        0xFA00_0000, // cube SRV to s[16:23] from s[0:1]
        0xF408_0300,
        0xFA00_0020, // raw output UAV to s[12:15] from s[0:1]+32
        0xF408_0601,
        0xFA00_0000, // sampler to s[24:27] from s[2:3]
        0x7E1E_0280, // v15 = 0 for output byte offsets
    ];
    let cases = [
        // image_sample_d: successive XY footprints (in texels).
        (0x22, 1.0, 0.0, 0.0),
        (0x22, 2.0, 0.0, 1.0),
        (0x22, 16.0, 0.0, 4.0),
        (0x22, 0.000001, 0.0, 0.0),
        (0x22, 0.0, 0.0, 0.0),
        // image_sample_d_cl: an instruction-specified minimum level.
        (0x23, 1.0, 3.0, 3.0),
        (0x23, 16.0, 2.0, 4.0),
        (0x23, 0.0, 2.0, 2.0),
        // image_sample_l and image_sample_lz: the level is given directly.
        (0x24, 0.0, 2.0, 2.0),
        (0x27, 0.0, 0.0, 0.0),
    ];
    for (i, &(op, footprint, lod, _)) in cases.iter().enumerate() {
        let delta = footprint / SIZE as f32;
        let mut addr = Vec::new();
        if op == 0x22 || op == 0x23 {
            addr.extend([delta, 0.0, 0.0, delta]); // derivatives along the face
        }
        addr.extend([1.5f32, 1.5, 0.0]); // positive X face, center
        if op == 0x23 || op == 0x24 {
            addr.push(lod);
        }
        for (reg, value) in addr.iter().enumerate() {
            words.extend([0x7E00_02FF | ((reg as u32) << 17), value.to_bits()]);
        }
        // MIMG, CUBE, dmask:R, v0... inputs, SRV s16, sampler s24, result v10.
        words.extend([0xF000_0118 | (op << 18), 0x00C4_0A00]);
        // buffer_store_dword v10, v15, s[12:15], 0 offen offset:i*4
        words.extend([0xE070_1000 | (i as u32 * 4), 0x8003_0A0F]);
    }
    words.push(0xBF81_0000); // s_endpgm
    let table = |slot, user_data| RootParam {
        root: RootType::Table,
        table_offset_dwords: 0,
        sgprs: 1,
        base_register: 0,
        user_data,
        slot,
    };
    let range = |kind, at, words, slot, user_data| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: at,
        lower_bound: 0,
        stride: 0,
        dwords: words,
        user_data,
        slot,
    };
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 16,
        layout: vec![table(0, 0), table(1, 2)],
        used: vec![
            range(ResourceType::Srv, 0, 8, 0, 0),
            range(ResourceType::Uav, 8, 4, 0, 0),
            range(ResourceType::Sampler, 0, 4, 1, 2),
        ],
        io: StageIo::Cs(ComputeIo {
            rsrc2: 3 << 1,
            threads: [1, 1, 1],
        }),
        code: words.into_iter().flat_map(u32::to_le_bytes).collect(),
    };
    let (shader, _) = build_compute_shader(&native).unwrap();
    let texels: Vec<_> = (0..6)
        .flat_map(|_| {
            (0..MIPS).flat_map(|mip| std::iter::repeat_n([f32::from(mip); 4], (SIZE >> mip).pow(2)))
        })
        .collect();
    let sampler = D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MaxLOD: f32::MAX,
        MaxAnisotropy: 1,
        ..Default::default()
    };
    for (name, warp) in [("WARP", true), ("hardware", false)] {
        let Ok(host) = Host::new(warp) else {
            eprintln!("no SM 6.6 {name} device; skipped");
            continue;
        };
        let (root, pso) = host.compute_pipeline(&shader).unwrap();
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(
                        0,
                        Root::Constants(vec![0; shader.user_data_dwords() as usize]),
                    )],
                    views: &[
                        View::Cube(SIZE as u32, MIPS, &texels),
                        View::RawUav(cases.len() as u32),
                    ],
                    samplers: &[sampler],
                    groups: [1, 1, 1],
                },
            )
            .unwrap();
        let levels: Vec<_> = out[0].iter().copied().map(f32::from_bits).collect();
        let expected = cases.map(|(_, _, _, mip)| mip);
        assert_eq!(levels.as_slice(), expected, "{name}");
    }
}

/// A comparison sampler over a depth texture (`image_sample_c_lz`, the title's most used sample
/// with 15 083 uses): the depth view is a D32_FLOAT texture on the Mac host. mac S6.
#[test]
#[cfg(target_os = "macos")]
fn comparison_sampler_reads_a_depth_texture() {
    const N: usize = 16;
    let mut words = vec![
        0xBE81_0380,
        0xBE83_0380, // s1, s3 = 0 (upper halves of the tables)
        0xF40C_0400,
        0xFA00_0000, // depth SRV to s[16:23] from s[0:1]
        0xF408_0300,
        0xFA00_0020, // raw output UAV to s[12:15] from s[0:1]+32
        0xF408_0601,
        0xFA00_0000, // sampler to s[24:27] from s[2:3]
        0x7E1E_0280, // v15 = 0 for output byte offsets
    ];
    let reference = |i: usize| (i % 5) as f32 * 0.25;
    for i in 0..N {
        let addr = [
            reference(i),
            ((i % 4) as f32 + 0.5) / 4.0,
            ((i / 4) as f32 + 0.5) / 4.0,
        ];
        for (reg, value) in addr.iter().enumerate() {
            words.extend([0x7E00_02FF | ((reg as u32) << 17), value.to_bits()]);
        }
        // MIMG, 2D, dmask:R, image_sample_c_lz v10, v[0:2], s[16:23], s[24:27]
        words.extend([0xF000_0108 | (0x2F << 18), 0x00C4_0A00]);
        // buffer_store_dword v10, v15, s[12:15], 0 offen offset:i*4
        words.extend([0xE070_1000 | (i as u32 * 4), 0x8003_0A0F]);
    }
    words.push(0xBF81_0000); // s_endpgm
    let table = |slot, user_data| RootParam {
        root: RootType::Table,
        table_offset_dwords: 0,
        sgprs: 1,
        base_register: 0,
        user_data,
        slot,
    };
    let range = |kind, at, words, slot, user_data| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: at,
        lower_bound: 0,
        stride: 0,
        dwords: words,
        user_data,
        slot,
    };
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 16,
        layout: vec![table(0, 0), table(1, 2)],
        used: vec![
            range(ResourceType::Srv, 0, 8, 0, 0),
            range(ResourceType::Uav, 8, 4, 0, 0),
            range(ResourceType::Sampler, 0, 4, 1, 2),
        ],
        io: StageIo::Cs(ComputeIo {
            rsrc2: 3 << 1,
            threads: [1, 1, 1],
        }),
        code: words.into_iter().flat_map(u32::to_le_bytes).collect(),
    };
    let (shader, _) = build_compute_shader(&native).unwrap();
    let depth: Vec<f32> = (0..N).map(|i| i as f32 / N as f32).collect();
    let sampler = D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER(0x80), // COMPARISON_MIN_MAG_MIP_POINT
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        ComparisonFunc: D3D12_COMPARISON_FUNC(4), // LESS_EQUAL
        MaxLOD: f32::MAX,
        MaxAnisotropy: 1,
        ..Default::default()
    };
    let Ok(host) = Host::new(false) else {
        eprintln!("no SM 6.6 device; skipped");
        return;
    };
    let (root, pso) = match host.compute_pipeline(&shader) {
        Err(e) if e.starts_with("unsupported") => {
            eprintln!("{e}; skipped");
            return;
        }
        made => made.unwrap(),
    };
    let out = host
        .dispatch(
            &root,
            &pso,
            &Dispatch {
                roots: vec![(
                    0,
                    Root::Constants(vec![0; shader.user_data_dwords() as usize]),
                )],
                views: &[View::Depth(4, 4, &depth), View::RawUav(N as u32)],
                samplers: &[sampler],
                groups: [1, 1, 1],
            },
        )
        .unwrap();
    let got: Vec<f32> = out[0].iter().copied().map(f32::from_bits).collect();
    let want: Vec<f32> = (0..N)
        .map(|i| if reference(i) <= depth[i] { 1.0 } else { 0.0 })
        .collect();
    assert_eq!(got, want);
}

// ---- mac S6: Texture1D / Texture2DArray / Texture3D SRVs and an RWTexture3D UAV on the executor ----

#[cfg(target_os = "macos")]
mod shaped {
    use super::*;

    const W: u32 = 4;
    const H: u32 = 3;
    const D: u32 = 2;

    /// MIMG dim field (bits 3..5 of the first word).
    #[derive(Clone, Copy)]
    enum Shape {
        D1 = 0,
        D3 = 2,
        Array = 5,
    }

    fn texel_value(x: u32, y: u32, z: u32) -> [f32; 4] {
        [
            (x + 10 * y + 100 * z) as f32,
            (x + 1) as f32 * 0.5,
            y as f32,
            z as f32 + 0.25,
        ]
    }

    fn native(words: Vec<u32>, uav: Option<u8>) -> NativeProgram {
        let table = |slot, user_data| RootParam {
            root: RootType::Table,
            table_offset_dwords: 0,
            sgprs: 1,
            base_register: 0,
            user_data,
            slot,
        };
        let range = |kind, at, words, slot, user_data| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords: at,
            lower_bound: 0,
            stride: 0,
            dwords: words,
            user_data,
            slot,
        };
        let mut used = vec![range(ResourceType::Srv, 0, 8, 0, 0)];
        used.push(match uav {
            Some(dw) => range(ResourceType::Uav, 8, dw, 0, 0),
            None => range(ResourceType::Uav, 8, 4, 0, 0),
        });
        used.push(range(ResourceType::Sampler, 0, 4, 1, 2));
        NativeProgram {
            stage: HwStage::Cs,
            wave64: false,
            vgprs: 16,
            layout: vec![table(0, 0), table(1, 2)],
            used,
            io: StageIo::Cs(ComputeIo {
                rsrc2: 3 << 1,
                threads: [1, 1, 1],
            }),
            code: words.into_iter().flat_map(u32::to_le_bytes).collect(),
        }
    }

    const PROLOGUE: [u32; 9] = [
        0xBE81_0380,
        0xBE83_0380,
        0xF40C_0400,
        0xFA00_0000, // SRV image to s[16:23]
        0xF408_0300,
        0xFA00_0020, // raw output UAV to s[12:15]
        0xF408_0601,
        0xFA00_0000, // sampler to s[24:27]
        0x7E1E_0280, // v15 = 0
    ];

    fn mov_imm(words: &mut Vec<u32>, reg: usize, bits: u32) {
        words.extend([0x7E00_02FF | ((reg as u32) << 17), bits]);
    }

    /// Loads and point samples of every texel of a shape; the red channel of each goes to the output.
    fn run(shape: Shape, size: [u32; 3], texels: &[[f32; 4]], view: View<'_>) {
        let dim = (shape as u32) << 3;
        let mut cases: Vec<(bool, [u32; 3])> = Vec::new();
        for z in 0..size[2] {
            for y in 0..size[1] {
                for x in 0..size[0] {
                    cases.push((false, [x, y, z]));
                    cases.push((true, [x, y, z]));
                }
            }
        }
        let mut words = PROLOGUE.to_vec();
        for (i, &(sample, c)) in cases.iter().enumerate() {
            let coords = match shape {
                Shape::D1 => 1,
                _ => 3,
            };
            for r in 0..coords {
                let v = if sample {
                    let extent = [size[0], size[1], size[2]][r] as f32;
                    // An array's layer is an integer index even when sampling.
                    if matches!(shape, Shape::Array) && r == 2 {
                        c[r] as f32
                    } else {
                        (c[r] as f32 + 0.5) / extent
                    }
                    .to_bits()
                } else {
                    c[r]
                };
                mov_imm(&mut words, r, v);
            }
            if sample {
                // image_sample_lz v10, v[0:2], s[16:23], s[24:27]
                words.extend([0xF000_0100 | dim | (0x27 << 18), 0x00C4_0A00]);
            } else {
                // image_load v10, v[0:2], s[16:23]
                words.extend([0xF000_0100 | dim, 0x0004_0A00]);
            }
            words.extend([0xBF8C_0F70]); // s_waitcnt vmcnt(0)
            words.extend([0xE070_1000 | (i as u32 * 4), 0x8003_0A0F]);
        }
        words.push(0xBF81_0000);
        let (shader, _) = build_compute_shader(&native(words, None)).unwrap();
        let sampler = D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_POINT,
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            MaxLOD: f32::MAX,
            MaxAnisotropy: 1,
            ..Default::default()
        };
        let Ok(host) = Host::new(false) else {
            eprintln!("no SM 6.6 device; skipped");
            return;
        };
        let (root, pso) = match host.compute_pipeline(&shader) {
            Err(e) if e.starts_with("unsupported") => {
                eprintln!("{e}; skipped");
                return;
            }
            made => made.unwrap(),
        };
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(
                        0,
                        Root::Constants(vec![0; shader.user_data_dwords() as usize]),
                    )],
                    views: &[view, View::RawUav(cases.len() as u32)],
                    samplers: &[sampler],
                    groups: [1, 1, 1],
                },
            )
            .unwrap();
        for (i, &(sample, c)) in cases.iter().enumerate() {
            let want = texels[(c[0] + size[0] * (c[1] + size[1] * c[2])) as usize][0];
            let got = f32::from_bits(out[0][i]);
            assert_eq!(got, want, "{} at {c:?}", if sample { "sample" } else { "load" });
        }
    }

    fn all(size: [u32; 3]) -> Vec<[f32; 4]> {
        let mut t = Vec::new();
        for z in 0..size[2] {
            for y in 0..size[1] {
                for x in 0..size[0] {
                    t.push(texel_value(x, y, z));
                }
            }
        }
        t
    }

    #[test]
    fn texture_1d_loads_and_samples() {
        let t = all([W, 1, 1]);
        run(Shape::D1, [W, 1, 1], &t, View::Texture1d(W, &t));
    }

    #[test]
    fn texture_2d_array_loads_and_samples() {
        let t = all([W, H, D]);
        run(Shape::Array, [W, H, D], &t, View::TextureArray(W, H, D, &t));
    }

    #[test]
    fn texture_3d_loads_and_samples() {
        let t = all([W, H, D]);
        run(Shape::D3, [W, H, D], &t, View::Texture3d(W, H, D, &t));
    }

    /// image_store into an RWTexture3D, read back by the executor: every texel gets its own value.
    #[test]
    fn rw_texture_3d_stores_come_back() {
        let mut words = vec![
            0xBE81_0380,
            0xBE83_0380,
            0xF40C_0400,
            0xFA00_0000, // UAV image to s[16:23]
        ];
        for z in 0..D {
            for y in 0..H {
                for x in 0..W {
                    for (r, v) in [x, y, z].into_iter().enumerate() {
                        mov_imm(&mut words, r, v);
                    }
                    for (r, v) in texel_value(x, y, z).into_iter().enumerate() {
                        mov_imm(&mut words, 10 + r, v.to_bits());
                    }
                    // image_store v[10:13], v[0:2], s[16:23] (dmask xyzw, 3D)
                    words.extend([0xF000_0F00 | ((Shape::D3 as u32) << 3) | (0x08 << 18), 0x0004_0A00]);
                }
            }
        }
        words.extend([0xBF8C_0F70, 0xBF81_0000]);
        let table = |slot, user_data| RootParam {
            root: RootType::Table,
            table_offset_dwords: 0,
            sgprs: 1,
            base_register: 0,
            user_data,
            slot,
        };
        let program = NativeProgram {
            stage: HwStage::Cs,
            wave64: false,
            vgprs: 16,
            layout: vec![table(0, 0)],
            used: vec![UsedResource {
                kind: ResourceType::Uav,
                root: RootType::Table,
                table_offset_dwords: 0,
                lower_bound: 0,
                stride: 0,
                dwords: 8,
                user_data: 0,
                slot: 0,
            }],
            io: StageIo::Cs(ComputeIo {
                rsrc2: 3 << 1,
                threads: [1, 1, 1],
            }),
            code: words.into_iter().flat_map(u32::to_le_bytes).collect(),
        };
        let (shader, _) = build_compute_shader(&program).unwrap();
        let Ok(host) = Host::new(false) else {
            eprintln!("no SM 6.6 device; skipped");
            return;
        };
        let (root, pso) = match host.compute_pipeline(&shader) {
            Err(e) if e.starts_with("unsupported") => {
                eprintln!("{e}; skipped");
                return;
            }
            made => made.unwrap(),
        };
        let out = host
            .dispatch(
                &root,
                &pso,
                &Dispatch {
                    roots: vec![(
                        0,
                        Root::Constants(vec![0; shader.user_data_dwords() as usize]),
                    )],
                    views: &[View::TextureUav3d(W, H, D)],
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap();
        let want: Vec<u32> = all([W, H, D])
            .into_iter()
            .flatten()
            .map(f32::to_bits)
            .collect();
        assert_eq!(out[0], want);
    }
}
