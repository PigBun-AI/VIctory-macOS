//! mac S6, oracle batch B: the image instructions (nodes `isa.mem.image_sample`,
//! `isa.mem.image_ldst`, `isa.mem.image_atomic`).
//!
//! One program per row: every case is a few `v_mov_b32` literals, the MIMG instruction, and the
//! four result registers stored to the output UAV (the technique of `cube_lod_tests`); the rows
//! are compared with a CPU model of D3D12 texture sampling.
//!
//! References: GFX10 (RDNA 2) ISA manual "Image Instructions" (MIMG, address VGPR order per
//! opcode: offset, bias, z-compare, derivatives, coordinates, lod/clamp), D3D11.3/D3D12 functional
//! specification: "Texture Addressing Modes", "Texture Sampling" (bilinear weights have at least
//! 8 fractional bits, so linear rows compare within 1/128), "Gather4" (texels returned as
//! (i0,j1), (i1,j1), (i1,j0), (i0,j0)), "Resource Load" and "Out of bounds addressing" (a load
//! or an atomic outside the view reads 0 and a store is dropped), and Mesa's `ac_nir` image
//! lowering for the argument order.

use super::host::api::*;
use super::host::{Dispatch, Host, Root, View};
use super::oracle_mem_tests::{S_ENDPGM, merge, opc, shader_of, store_out};
use super::oracle_tests::{Outcome, report_family};
use super::valu_tests::ONE_AT_A_TIME;
use super::HostShader;
use crate::native::{
    ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
};

const W: usize = 4;
const H: usize = 4;

/// `image_<op>` dim 2D, MIMG: v[vaddr...], dest v10..., image s[16:23], sampler s[24:27].
fn mimg(name: &str, dmask: u32, vaddr: u8, glc: bool, unorm: bool) -> [u32; 2] {
    [
        0xF000_0000
            | opc(name) << 18
            | u32::from(glc) << 13
            | u32::from(unorm) << 12
            | dmask << 8
            | 1 << 3, // 2D
        u32::from(vaddr) | 10 << 8 | (16 >> 2) << 16 | (24 >> 2) << 21,
    ]
}

fn vmov(reg: u8, bits: u32) -> [u32; 2] {
    [0x7E00_02FF | u32::from(reg) << 17, bits]
}

/// What the table at descriptor 0 is: a sampled image, or an image UAV (the output UAV follows).
#[derive(Clone, Copy, PartialEq)]
enum Img {
    Srv,
    Uav,
}

fn program(img: Img, body: &[u32], with_sampler: bool) -> Result<HostShader, String> {
    let mut words = vec![
        0xBE81_0380u32, // s_mov_b32 s1, 0
        0xBE83_0380,    // s_mov_b32 s3, 0
        0xF40C_0400,    // s_load_dwordx8 s[16:23], s[0:1], 0
        0xFA00_0000,
        0xF408_0300, // s_load_dwordx4 s[12:15], s[0:1], 0x20
        0xFA00_0020,
    ];
    if with_sampler {
        words.extend([0xF408_0601, 0xFA00_0000]); // s_load_dwordx4 s[24:27], s[2:3], 0
    }
    words.extend([0x7E1E_0280]); // v_mov_b32 v15, 0
    words.extend_from_slice(body);
    words.push(S_ENDPGM);
    let table = |slot, user_data| RootParam {
        root: RootType::Table,
        table_offset_dwords: 0,
        sgprs: 1,
        base_register: 0,
        user_data,
        slot,
    };
    let range = |kind, at, dwords, slot, user_data, lower_bound| UsedResource {
        kind,
        root: RootType::Table,
        table_offset_dwords: at,
        lower_bound,
        stride: 0,
        dwords,
        user_data,
        slot,
    };
    let mut used = vec![
        match img {
            Img::Srv => range(ResourceType::Srv, 0, 8, 0, 0, 0),
            Img::Uav => range(ResourceType::Uav, 0, 8, 0, 0, 0),
        },
        range(ResourceType::Uav, 8, 4, 0, 0, if img == Img::Uav { 1 } else { 0 }),
    ];
    if with_sampler {
        used.push(range(ResourceType::Sampler, 0, 4, 1, 2, 0));
    }
    let native = NativeProgram {
        stage: HwStage::Cs,
        wave64: false,
        vgprs: 24,
        layout: vec![table(0, 0), table(1, 2)],
        used,
        io: StageIo::Cs(ComputeIo { rsrc2: 3 << 1, threads: [1, 1, 1] }),
        code: words.into_iter().flat_map(u32::to_le_bytes).collect(),
    };
    shader_of(&native)
}

/// The 4x4 RGBA32F texture the sampling rows read: all channels differ per texel, below 1.
fn texels() -> Vec<[f32; 4]> {
    (0..W * H)
        .map(|i| {
            let f = i as f32;
            [(f + 1.0) / 32.0, (f + 17.0) / 64.0, 1.0 - f / 20.0, f / 40.0 + 0.25]
        })
        .collect()
}

fn texel(x: i64, y: i64, wrap: bool) -> [f32; 4] {
    let t = texels();
    let (x, y) = if wrap { (x.rem_euclid(W as i64), y.rem_euclid(H as i64)) } else { (x.clamp(0, W as i64 - 1), y.clamp(0, H as i64 - 1)) };
    t[y as usize * W + x as usize]
}

#[derive(Clone, Copy)]
struct Smp {
    linear: bool,
    wrap: bool,
    compare: bool,
}

fn sampler_desc(s: Smp) -> D3D12_SAMPLER_DESC {
    let mode = if s.wrap { D3D12_TEXTURE_ADDRESS_MODE_WRAP } else { D3D12_TEXTURE_ADDRESS_MODE_CLAMP };
    let filter = match (s.linear, s.compare) {
        (false, false) => D3D12_FILTER_MIN_MAG_MIP_POINT,
        (true, false) => D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        (false, true) => D3D12_FILTER(0x80),
        (true, true) => D3D12_FILTER(0x95),
    };
    D3D12_SAMPLER_DESC {
        Filter: filter,
        AddressU: mode,
        AddressV: mode,
        AddressW: mode,
        MaxLOD: f32::MAX,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC(4), // LESS_EQUAL
        ..Default::default()
    }
}

/// The sample of `s` at (u, v), channels 0..4 (the compare value `z` against red when `s.compare`).
fn model_sample(s: Smp, u: f32, v: f32, off: (i64, i64), z: f32) -> [f32; 4] {
    let (fx, fy) = (u * W as f32 - 0.5, v * H as f32 - 0.5);
    let tex = |x: i64, y: i64| {
        let t = texel(x + off.0, y + off.1, s.wrap);
        if s.compare { [if z <= t[0] { 1.0 } else { 0.0 }; 4] } else { t }
    };
    if !s.linear {
        let (x, y) = ((u * W as f32).floor() as i64, (v * H as f32).floor() as i64);
        return tex(x, y);
    }
    let (x0, y0) = (fx.floor(), fy.floor());
    let (ax, ay) = (fx - x0, fy - y0);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let mut out = [0.0f32; 4];
    for (dx, dy, wgt) in [(0, 0, (1.0 - ax) * (1.0 - ay)), (1, 0, ax * (1.0 - ay)), (0, 1, (1.0 - ax) * ay), (1, 1, ax * ay)] {
        let t = tex(x0 + dx, y0 + dy);
        for c in 0..4 {
            out[c] += t[c] * wgt;
        }
    }
    out
}

/// The gather of channel `ch`: texels (i0,j1), (i1,j1), (i1,j0), (i0,j0).
fn model_gather(s: Smp, u: f32, v: f32, off: (i64, i64), ch: usize, z: f32) -> [f32; 4] {
    let (x0, y0) = ((u * W as f32 - 0.5).floor() as i64 + off.0, (v * H as f32 - 0.5).floor() as i64 + off.1);
    let t = |x: i64, y: i64| {
        let t = texel(x, y, s.wrap);
        if s.compare { if z <= t[0] { 1.0 } else { 0.0 } } else { t[ch] }
    };
    [t(x0, y0 + 1), t(x0 + 1, y0 + 1), t(x0 + 1, y0), t(x0, y0)]
}

#[derive(Clone, Copy)]
enum Kind {
    Sample,
    Gather,
}

/// One sampling row: the address VGPRs of a case (in the instruction's order).
struct SampleRow {
    name: &'static str,
    /// Address registers (as floats), from the case's (u, v, z, offset).
    addr: fn(u: f32, v: f32, z: f32, off: (i64, i64)) -> Vec<u32>,
    kind: Kind,
    compare: bool,
    offsets: bool,
    dmask: u32,
}

const COORDS: &[(f32, f32)] = &[
    (0.125, 0.125),
    (0.5, 0.5),
    (0.3, 0.7),
    (0.9, 0.2),
    (0.0, 0.0),
    (0.99, 0.99),
    (-0.2, 0.4),
    (1.3, 0.6),
    (0.4, -0.35),
    (2.75, 1.1),
];

fn off_bits(o: (i64, i64)) -> u32 {
    (o.0 as u32 & 0x3F) | (o.1 as u32 & 0x3F) << 8
}

fn run_sample_row(r: &SampleRow) -> Outcome {
    let mut parts = Vec::new();
    for (name, smp) in [
        ("point clamp", Smp { linear: false, wrap: false, compare: r.compare }),
        ("point wrap", Smp { linear: false, wrap: true, compare: r.compare }),
        ("linear clamp", Smp { linear: true, wrap: false, compare: r.compare }),
        ("linear wrap", Smp { linear: true, wrap: true, compare: r.compare }),
    ] {
        // shortcut (lower.rs, `image_gather4_l`): four texel reads at the named level, the edge
        // clamped whatever the sampler says; the wrap forms are not asked of it.
        if smp.wrap && r.name.starts_with("image_gather4_l") && !r.name.contains("_lz") {
            continue;
        }
        let offsets: &[(i64, i64)] = if r.offsets { &[(0, 0), (1, 0), (-1, 2), (3, -2)] } else { &[(0, 0)] };
        let mut cases = Vec::new();
        for &(u, v) in COORDS {
            for &o in offsets {
                cases.push((u, v, o));
            }
        }
        let z = 0.2f32;
        let mut body = Vec::new();
        for (i, &(u, v, o)) in cases.iter().enumerate() {
            for (k, bits) in (r.addr)(u, v, z, o).into_iter().enumerate() {
                body.extend(vmov(k as u8, bits));
            }
            body.extend(mimg(r.name, r.dmask, 0, false, false));
            for k in 0..4u8 {
                body.extend(store_out(10 + k, (16 * i as u16) + 4 * u16::from(k)));
            }
        }
        let shader = match program(Img::Srv, &body, true) {
            Ok(s) => s,
            Err(e) => {
                parts.push((name.to_string(), Outcome::Untranslated(e)));
                continue;
            }
        };
        let o = run_image(&shader, &[sampler_desc(smp)], cases.len(), smp.compare, |got| {
            let mut bad = None;
            for (i, &(u, v, o)) in cases.iter().enumerate() {
                let want = match r.kind {
                    Kind::Sample => model_sample(smp, u, v, o, z),
                    Kind::Gather => model_gather(smp, u, v, o, r.dmask.trailing_zeros() as usize, z),
                };
                let have: Vec<f32> = got[0][4 * i..4 * i + 4].iter().map(|&b| f32::from_bits(b)).collect();
                let n = if matches!(r.kind, Kind::Sample) { r.dmask.count_ones() as usize } else { 4 };
                let tol = if smp.linear && !matches!(r.kind, Kind::Gather) { 0.012 } else { 0.0 };
                // dmask 0xF: channels in order; a single channel lands in the first register
                let ok = if matches!(r.kind, Kind::Sample) && r.dmask != 0xF {
                    let c = r.dmask.trailing_zeros() as usize;
                    (have[0] - want[c]).abs() <= tol
                } else {
                    (0..n).all(|c| (have[c] - want[c]).abs() <= tol)
                };
                if !ok && bad.is_none() {
                    bad = Some(format!("(u {u}, v {v}, off {o:?}): got {have:?} want {want:?}"));
                }
            }
            bad
        });
        parts.push((name.to_string(), o));
    }
    merge(parts)
}

/// Dispatches a sampled-image program with the 4x4 texture; `check` sees the output UAV.
fn run_image(shader: &HostShader, samplers: &[D3D12_SAMPLER_DESC], cases: usize, depth: bool, mut check: impl FnMut(&[Vec<u32>]) -> Option<String>) -> Outcome {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        return Outcome::Refused("no device".into());
    };
    let (root, pso) = match host.compute_pipeline(shader) {
        Ok(p) => p,
        Err(e) if e.contains("unsupported") => return Outcome::Refused(e),
        Err(e) => return Outcome::Mismatch { cases: 0, bad: 1, examples: vec![format!("compute_pipeline: {e}")] },
    };
    let t = texels();
    // a comparison sampler reads a depth texture: the red channel of the colour texels
    let reds: Vec<f32> = t.iter().map(|x| x[0]).collect();
    let first = if depth { View::Depth(W as u32, H as u32, &reds) } else { View::Texture(W as u32, H as u32, &t) };
    let got = host.dispatch(
        &root,
        &pso,
        &Dispatch {
            roots: vec![(0, Root::Constants(vec![0; shader.user_data_dwords().max(1) as usize]))],
            views: &[first, View::RawUav(4 * cases as u32)],
            samplers,
            groups: [1, 1, 1],
        },
    );
    match got {
        Err(e) if e.contains("unsupported") => Outcome::Refused(e),
        Err(e) => Outcome::Mismatch { cases: 0, bad: 1, examples: vec![format!("dispatch: {e}")] },
        Ok(g) => match check(&g) {
            None => Outcome::Passed(cases),
            Some(t) => Outcome::Mismatch { cases, bad: 1, examples: vec![t] },
        },
    }
}

fn f(x: f32) -> u32 {
    x.to_bits()
}

fn sample_rows() -> Vec<SampleRow> {
    let s = |name, addr: fn(f32, f32, f32, (i64, i64)) -> Vec<u32>, compare, offsets| SampleRow {
        name,
        addr,
        kind: Kind::Sample,
        compare,
        offsets,
        // a comparison returns one channel
        dmask: if compare { 1 } else { 0xF },
    };
    let g = |name, addr: fn(f32, f32, f32, (i64, i64)) -> Vec<u32>, compare, dmask| SampleRow {
        name,
        addr,
        kind: Kind::Gather,
        compare,
        offsets: name.ends_with("_o"),
        dmask,
    };
    // Rows are the mnemonics of the census (`isa-mnemonics.tsv`) the translator takes in a compute
    // program. The comparison rows read a depth texture (`View::Depth`, the red channel of the
    // colour texels) through a comparison sampler (filter 0x80 point, 0x95 linear: PCF of the
    // four comparisons; LESS_EQUAL against z = 0.2). Not here: `image_get_lod` /
    // `image_sample_c` (implicit derivatives: pixel programs only); `image_sample_c_lz_o`, `image_gather4_c_lz`
    // and `image_sample_c_l[_o]` are refused by the translator and no program of the census has them.
    vec![
        s("image_sample_lz", |u, v, _, _| vec![f(u), f(v)], false, false),
        s("image_sample_l", |u, v, _, _| vec![f(u), f(v), f(0.0)], false, false),
        s("image_sample", |u, v, _, _| vec![f(u), f(v)], false, false),
        s("image_sample_b", |u, v, _, _| vec![f(0.0), f(u), f(v)], false, false),
        s("image_sample_cl", |u, v, _, _| vec![f(u), f(v), f(0.0)], false, false),
        s("image_sample_d", |u, v, _, _| vec![f(0.0), f(0.0), f(0.0), f(0.0), f(u), f(v)], false, false),
        s("image_sample_d_cl", |u, v, _, _| vec![f(0.0), f(0.0), f(0.0), f(0.0), f(u), f(v), f(0.0)], false, false),
        s("image_sample_lz_o", |u, v, _, o| vec![off_bits(o), f(u), f(v)], false, true),
        s("image_sample_o", |u, v, _, o| vec![off_bits(o), f(u), f(v)], false, true),
        g("image_gather4_lz", |u, v, _, _| vec![f(u), f(v)], false, 1),
        g("image_gather4_lz", |u, v, _, _| vec![f(u), f(v)], false, 4),
        g("image_gather4_lz_o", |u, v, _, o| vec![off_bits(o), f(u), f(v)], false, 1),
        s("image_sample_c_lz", |u, v, z, _| vec![f(z), f(u), f(v)], true, false),
        g("image_gather4_l", |u, v, _, _| vec![f(u), f(v), f(0.0)], false, 1),
        g("image_gather4_l_o", |u, v, _, o| vec![off_bits(o), f(u), f(v), f(0.0)], false, 1),
    ]
}

fn sample_family() -> Vec<(String, Outcome)> {
    let mut seen = std::collections::BTreeSet::new();
    sample_rows()
        .iter()
        .map(|r| {
            // a second dmask of the same mnemonic is named with it
            let label = if seen.insert(r.name) { r.name.to_string() } else { format!("{}#c", r.name) };
            (label, run_sample_row(r))
        })
        .collect()
}

// ---- image_load, image_load_mip, image_get_resinfo, image_get_lod from a sampled image ----

fn load_rows() -> Vec<(String, Outcome)> {
    let mut rows = Vec::new();
    // integer coordinates inside and outside the 4x4 view (outside reads 0)
    let coords: Vec<(i32, i32)> = vec![(0, 0), (3, 0), (1, 2), (3, 3), (4, 0), (0, 4), (-1, 1), (7, 7)];
    for (name, mip) in [("image_load", false), ("image_load_mip", true)] {
        let mut body = Vec::new();
        for (i, &(x, y)) in coords.iter().enumerate() {
            body.extend(vmov(0, x as u32));
            body.extend(vmov(1, y as u32));
            if mip {
                body.extend(vmov(2, 0));
            }
            body.extend(mimg(name, 0xF, 0, false, false));
            for k in 0..4u8 {
                body.extend(store_out(10 + k, (16 * i as u16) + 4 * u16::from(k)));
            }
        }
        let o = match program(Img::Srv, &body, false) {
            Err(e) => Outcome::Untranslated(e),
            Ok(shader) => run_image(&shader, &[], coords.len(), false, |got| {
                for (i, &(x, y)) in coords.iter().enumerate() {
                    let inside = (0..W as i32).contains(&x) && (0..H as i32).contains(&y);
                    let want = if inside { texel(x.into(), y.into(), false) } else { [0.0; 4] };
                    let have: Vec<f32> = got[0][4 * i..4 * i + 4].iter().map(|&b| f32::from_bits(b)).collect();
                    if have != want {
                        return Some(format!("({x}, {y}): got {have:?} want {want:?}"));
                    }
                }
                None
            }),
        };
        rows.push((name.to_string(), o));
    }
    // resinfo: width, height, depth, mip count (mip level in v0)
    {
        let mut body = vmov(0, 0).to_vec();
        body.extend(mimg("image_get_resinfo", 0xF, 0, false, false));
        for k in 0..4u8 {
            body.extend(store_out(10 + k, 4 * u16::from(k)));
        }
        let o = match program(Img::Srv, &body, false) {
            Err(e) => Outcome::Untranslated(e),
            Ok(shader) => run_image(&shader, &[], 1, false, |got| {
                let want = [W as u32, H as u32, 0, 1];
                // the depth of a 2D view is not read (dmask bit 2 reads 0 or 1)
                (got[0][0] != want[0] || got[0][1] != want[1] || got[0][3] != want[3])
                    .then(|| format!("got {:x?} want {want:x?}", &got[0][..4]))
            }),
        };
        rows.push(("image_get_resinfo".to_string(), o));
    }
    // get_lod needs derivatives: the translator takes it in pixel programs only (a compute
    // program has none), where this host has no draw: not run here (see the matrix).
    if false {
        let mut body = Vec::new();
        body.extend(vmov(0, f(0.4)));
        body.extend(vmov(1, f(0.6)));
        body.extend(mimg("image_get_lod", 0x3, 0, false, false));
        for k in 0..2u8 {
            body.extend(store_out(10 + k, 4 * u16::from(k)));
        }
        let o = match program(Img::Srv, &body, true) {
            Err(e) => Outcome::Untranslated(e),
            Ok(shader) => run_image(&shader, &[sampler_desc(Smp { linear: true, wrap: false, compare: false })], 1, false, |got| {
                let have = [f32::from_bits(got[0][0]), f32::from_bits(got[0][1])];
                (have[0] != 0.0).then(|| format!("got {have:?}, want a clamped lod of 0"))
            }),
        };
        rows.push(("image_get_lod".to_string(), o));
    }
    rows
}

// ---- UAV images: image_store, image_load, image_atomic_* ----

fn run_uav(shader: &HostShader, cases: usize, want_image: &[u32], mut check_out: impl FnMut(&[u32]) -> Option<String>) -> Outcome {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(host) = Host::new(false) else {
        return Outcome::Refused("no device".into());
    };
    let (root, pso) = match host.compute_pipeline(shader) {
        Ok(p) => p,
        Err(e) if e.contains("unsupported") => return Outcome::Refused(e),
        Err(e) => return Outcome::Mismatch { cases: 0, bad: 1, examples: vec![format!("compute_pipeline: {e}")] },
    };
    let uint = shader_view_is_uint(want_image);
    let views = [
        if uint { View::UintTextureUav(W as u32, H as u32) } else { View::TextureUav(W as u32, H as u32) },
        View::RawUav(4 * cases as u32),
    ];
    let got = host.dispatch(
        &root,
        &pso,
        &Dispatch {
            roots: vec![(0, Root::Constants(vec![0; shader.user_data_dwords().max(1) as usize]))],
            views: &views,
            samplers: &[],
            groups: [1, 1, 1],
        },
    );
    match got {
        Err(e) if e.contains("unsupported") => Outcome::Refused(e),
        Err(e) => Outcome::Mismatch { cases: 0, bad: 1, examples: vec![format!("dispatch: {e}")] },
        Ok(g) => {
            let stride = if uint { 1 } else { 4 };
            let image_ok = want_image.is_empty() || g[0].iter().step_by(stride).copied().take(want_image.len()).collect::<Vec<_>>()[..] == want_image[..];
            if let Some(t) = check_out(&g[1]) {
                return Outcome::Mismatch { cases, bad: 1, examples: vec![t] };
            }
            if !image_ok {
                return Outcome::Mismatch { cases, bad: 1, examples: vec![format!("image: got {:x?}, want red channels {want_image:x?}", &g[0][..g[0].len().min(64)])] };
            }
            Outcome::Passed(cases)
        }
    }
}

/// Whether the image is the R32_UINT kind (an atomic row passes its expected texels as dwords with
/// the top bit of the marker set; here every UAV image is float unless asked).
fn shader_view_is_uint(_want: &[u32]) -> bool {
    UINT_VIEW.with(|u| u.get())
}

thread_local! {
    static UINT_VIEW: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn uav_rows() -> Vec<(String, Outcome)> {
    let mut rows = Vec::new();
    UINT_VIEW.with(|u| u.set(false));
    // image_store of four channels at in-range and out-of-range texels, then image_load of them
    {
        let pts: Vec<(u32, u32)> = vec![(0, 0), (2, 1), (3, 3), (4, 1), (1, 9)];
        let vals = |i: usize| [f(0.25 + i as f32), f(-0.5 * i as f32), f(2.0), f(i as f32 / 8.0)];
        let mut body = Vec::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            body.extend(vmov(0, x));
            body.extend(vmov(1, y));
            for (k, v) in vals(i).into_iter().enumerate() {
                body.extend(vmov(10 + k as u8, v));
            }
            // image_store v[10:13], v[0:1], s[16:23] dmask 0xF
            body.extend(mimg("image_store", 0xF, 0, false, false));
            body.extend(vmov(10, 0));
            body.extend(vmov(11, 0));
            body.extend(vmov(12, 0));
            body.extend(vmov(13, 0));
            body.extend(mimg("image_load", 0xF, 0, false, false));
            for k in 0..4u8 {
                body.extend(store_out(10 + k, (16 * i as u16) + 4 * u16::from(k)));
            }
        }
        let o = match program(Img::Uav, &body, false) {
            Err(e) => Outcome::Untranslated(e),
            Ok(shader) => {
                let mut want_image = vec![0u32; W * H];
                for (i, &(x, y)) in pts.iter().enumerate() {
                    if (x as usize) < W && (y as usize) < H {
                        want_image[y as usize * W + x as usize] = vals(i)[0];
                    }
                }
                run_uav(&shader, pts.len(), &want_image, |out| {
                    for (i, &(x, y)) in pts.iter().enumerate() {
                        let inside = (x as usize) < W && (y as usize) < H;
                        let want = if inside { vals(i) } else { [0; 4] };
                        if out[4 * i..4 * i + 4] != want {
                            return Some(format!("({x}, {y}): loaded {:x?} want {want:x?}", &out[4 * i..4 * i + 4]));
                        }
                    }
                    None
                })
            }
        };
        rows.push(("image_store".to_string(), o));
    }
    rows
}

/// The image atomics on an R32_UINT image: the texel (1, 1) starts at 0; the program runs the
/// operation twice (operands c0, c1), returning the old values; compared with the model.
fn atomic_rows() -> Vec<(String, Outcome)> {
    UINT_VIEW.with(|u| u.set(true));
    type Op = fn(u32, u32) -> u32;
    let rows: [(&'static str, Op); 8] = [
        ("image_atomic_add", |m, d| m.wrapping_add(d)),
        ("image_atomic_or", |m, d| m | d),
        ("image_atomic_and", |m, d| m & d),
        ("image_atomic_xor", |m, d| m ^ d),
        ("image_atomic_umin", |m, d| m.min(d)),
        ("image_atomic_umax", |m, d| m.max(d)),
        ("image_atomic_smax", |m, d| (m as i32).max(d as i32) as u32),
        ("image_atomic_swap", |_, d| d),
    ];
    let (a, b) = (0x8000_0003u32, 0x0000_00F0u32);
    let mut out = Vec::new();
    for (name, f) in rows {
        let mut body = Vec::new();
        for (i, d) in [a, b].into_iter().enumerate() {
            body.extend(vmov(10, d)); // data
            body.extend(vmov(0, 1)); // x
            body.extend(vmov(1, 1)); // y
            body.extend(mimg(name, 1, 0, true, false));
            body.extend(store_out(10, 4 * i as u16));
        }
        let o = match program(Img::Uav, &body, false) {
            Err(e) => Outcome::Untranslated(e),
            Ok(shader) => {
                let m1 = f(0, a);
                let m2 = f(m1, b);
                let mut want_image = vec![0u32; W * H];
                want_image[W + 1] = m2;
                run_uav(&shader, 2, &want_image, |o| (o[..2] != [0, m1]).then(|| format!("old values {:x?}, want {:x?}", &o[..2], [0, m1])))
            }
        };
        out.push((name.to_string(), o));
    }
    // cmpswap: (replacement, comparand) in v[10:11]
    {
        let mut body = Vec::new();
        for (i, (new, cmp)) in [(5u32, 0u32), (9, 1), (7, 5)].into_iter().enumerate() {
            body.extend(vmov(10, new));
            body.extend(vmov(11, cmp));
            body.extend(vmov(0, 1));
            body.extend(vmov(1, 1));
            body.extend(mimg("image_atomic_cmpswap", 3, 0, true, false));
            body.extend(store_out(10, 4 * i as u16));
        }
        let o = match program(Img::Uav, &body, false) {
            Err(e) => Outcome::Untranslated(e),
            Ok(shader) => {
                let mut want_image = vec![0u32; W * H];
                want_image[W + 1] = 7; // 0 -> 5; 5 != 1 stays; 5 == 5 -> 7
                run_uav(&shader, 3, &want_image, |o| (o[..3] != [0, 5, 5]).then(|| format!("old values {:x?}, want [0, 5, 5]", &o[..3])))
            }
        };
        out.push(("image_atomic_cmpswap".to_string(), o));
    }
    UINT_VIEW.with(|u| u.set(false));
    out
}

fn rows_test(family: &str, rows: impl FnOnce() -> Vec<(String, Outcome)>) {
    if Host::new(false).is_err() {
        eprintln!("{family}: no device; not run");
        return;
    }
    report_family(family, rows());
}

#[test]
fn oracle_image_sample() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("image_sample", sample_family);
}

#[test]
fn oracle_image_load() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("image_load", load_rows);
}

#[test]
fn oracle_image_uav() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("image_uav", uav_rows);
}

#[test]
fn oracle_image_atomic() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("image_atomic", atomic_rows);
}

#[test]
fn every_image_row_names_a_gfx10_mnemonic() {
    let _chain = super::oracle_tests::metal_chain();
    for r in sample_rows() {
        assert!(crate::gfx10::opcode_of(r.name).is_some(), "{}", r.name);
    }
}
