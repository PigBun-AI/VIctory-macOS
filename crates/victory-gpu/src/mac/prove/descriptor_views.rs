//! C-V-012, the host's side (docs/mac/PATCHES.md G12): the title's view description, as a record in its descriptor
//! memory, becomes a host view that a shader reads the texel the description says. A child module of
//! `descriptor::host` (`#[path]`) for its private items; the GPU is the host's (D3DMetal under Wine, or the Metal
//! back), the expected texels are computed here from the D3D12 rules.
//!
//! Reference: the D3D12 functional specification, "Resource views": `D3D12_SHADER_RESOURCE_VIEW_DESC` (format, dimension,
//! `Shader4ComponentMapping`, the union per dimension: `MostDetailedMip`, `MipLevels`, `FirstArraySlice`, `ArraySize`,
//! `First2DArrayFace`, `NumCubes`), `Load(location, mip)` / `SampleLevel` reading the subresource the view selects,
//! `D3D12_ENCODE_SHADER_4_COMPONENT_MAPPING` (0-3 pick a source component, 4 forces 0, 5 forces 1; bit 12 always set),
//! and missing components read as (0, 0, 0, 1) (D3D11.3 functional spec, "Format conversion for reads"). A null
//! descriptor (null resource, description given) reads zeros (ResourceBinding.md, "NULL Descriptors").
use super::*;
use crate::mac::prove::rig::Rig;
use crate::object::{This, S_OK};
use crate::resource::{HeapProperties, ResourceDesc};

/// `D3D12_SHADER_RESOURCE_VIEW_DESC` is 40 bytes: format, dimension, mapping, a pad (the union is 8-aligned), 24 bytes
/// of union (the console passes it as these ten words).
#[test]
fn the_view_descriptions_have_the_hosts_layout() {
    assert_eq!(size_of::<D3D12_SHADER_RESOURCE_VIEW_DESC>(), 40);
    assert_eq!(size_of::<D3D12_UNORDERED_ACCESS_VIEW_DESC>(), 40);
    let d = srv_desc(&[28, 4, 5768, 0, 7, 3, 1, 0x3f80_0000, 0, 0]);
    assert_eq!((d.Format.0, d.ViewDimension.0, d.Shader4ComponentMapping), (28, 4, 5768));
    // SAFETY: the member its dimension says.
    let t = unsafe { d.Anonymous.Texture2D };
    assert_eq!((t.MostDetailedMip, t.MipLevels, t.PlaneSlice, t.ResourceMinLODClamp), (7, 3, 1, 1.0));
    // the unordered access description's union is 8-aligned as well but follows the dimension directly: 8 + 24 = 32 bytes padded to 40
    let u = uav_desc(&[28, 4, 5, 2, 0, 0, 0, 0, 0, 0]);
    assert_eq!((u.Format.0, u.ViewDimension.0), (28, 4));
    // SAFETY: the member its dimension says.
    assert_eq!(unsafe { (u.Anonymous.Texture2D.MipSlice, u.Anonymous.Texture2D.PlaneSlice) }, (5, 2));
    // the console's 16-bit depth with stencil is the 24-bit format on the host (resource.rs pc_format)
    for (console, host) in [(118, 45), (119, 46), (120, 47), (28, 28), (45, 45), (0, 0), (115, 115)] {
        assert_eq!(srv_desc(&[console, 4, 5768, 0, 0, 1, 0, 0, 0, 0]).Format.0, host, "format {console}");
    }
}

fn texel_byte(mip: u32, layer: u32, y: u32, x: u32, k: u32) -> u8 {
    let h = (mip * 0x9e37 + layer * 0x85eb + y * 0xc2b2 + x * 0x27d4 + k * 0x1657 + 11).wrapping_mul(2_654_435_761);
    (h >> 13) as u8
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Uint,
    Unorm,
}

/// A texture format the cases use: DXGI number, channels, bytes per channel, kind.
#[derive(Clone, Copy)]
struct Fmt {
    dxgi: u32,
    channels: u32,
    bytes: u32,
    kind: Kind,
}
const R8G8B8A8_UNORM: Fmt = Fmt { dxgi: 28, channels: 4, bytes: 1, kind: Kind::Unorm };
const R8G8B8A8_UINT: Fmt = Fmt { dxgi: 30, channels: 4, bytes: 1, kind: Kind::Uint };
const R32_UINT: Fmt = Fmt { dxgi: 42, channels: 1, bytes: 4, kind: Kind::Uint };
const R16G16_UINT: Fmt = Fmt { dxgi: 36, channels: 2, bytes: 2, kind: Kind::Uint };
const R32G32B32A32_UINT: Fmt = Fmt { dxgi: 3, channels: 4, bytes: 4, kind: Kind::Uint };
const R8_UINT: Fmt = Fmt { dxgi: 62, channels: 1, bytes: 1, kind: Kind::Uint };

/// A resource and its view as the title would describe them.
struct Case {
    name: &'static str,
    /// Console resource dimension: 2 = 1D, 3 = 2D, 4 = 3D.
    resource: u32,
    format: Fmt,
    width: u32,
    height: u32,
    /// Depth of a 3D texture, else the array size.
    depth: u32,
    mips: u32,
    /// `D3D12_SRV_DIMENSION`.
    dimension: u32,
    /// The union's words after the pad: written by the case.
    union: [u32; 6],
    mapping: u32,
    /// The view starts at this mip and slice (as `MostDetailedMip`, `FirstArraySlice` or `First2DArrayFace` say), what
    /// the probes are relative to.
    first_mip: u32,
    first_layer: u32,
    /// (x, y, layer in the view, mip in the view), or for cubes (face, 0, cube, mip).
    probes: &'static [[u32; 4]],
}

const fn mapping(r: u32, g: u32, b: u32, a: u32) -> u32 {
    (r & 7) | (g & 7) << 3 | (b & 7) << 6 | (a & 7) << 9 | 1 << 12
}
const DEFAULT: u32 = mapping(0, 1, 2, 3);
const _: () = assert!(DEFAULT == 5768);

fn cases() -> Vec<Case> {
    let probes2d: &[[u32; 4]] = &[[0, 0, 0, 0], [5, 2, 0, 0], [7, 7, 0, 0], [1, 3, 0, 1], [3, 0, 0, 1], [0, 0, 0, 2]];
    vec![
        Case { name: "1D", resource: 2, format: R32_UINT, width: 16, height: 1, depth: 1, mips: 3, dimension: 2, union: [0, 3, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 0, probes: &[[0, 0, 0, 0], [15, 0, 0, 0], [5, 0, 0, 1], [3, 0, 0, 2]] },
        Case { name: "1D from mip 1", resource: 2, format: R8G8B8A8_UINT, width: 16, height: 1, depth: 1, mips: 4, dimension: 2, union: [1, 3, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 1, first_layer: 0, probes: &[[0, 0, 0, 0], [7, 0, 0, 0], [2, 0, 0, 1]] },
        Case { name: "1D array", resource: 2, format: R16G16_UINT, width: 8, height: 1, depth: 4, mips: 2, dimension: 3, union: [0, 2, 1, 3, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 1, probes: &[[0, 0, 0, 0], [7, 0, 2, 0], [3, 0, 1, 1]] },
        Case { name: "2D", resource: 3, format: R8G8B8A8_UINT, width: 8, height: 8, depth: 1, mips: 4, dimension: 4, union: [0, 4, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 0, probes: probes2d },
        Case { name: "2D unorm", resource: 3, format: R8G8B8A8_UNORM, width: 8, height: 8, depth: 1, mips: 4, dimension: 4, union: [0, 4, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 0, probes: probes2d },
        Case { name: "2D from mip 2", resource: 3, format: R32_UINT, width: 16, height: 8, depth: 1, mips: 5, dimension: 4, union: [2, 3, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 2, first_layer: 0, probes: &[[0, 0, 0, 0], [3, 1, 0, 0], [1, 0, 0, 1]] },
        Case { name: "2D one level of many", resource: 3, format: R16G16_UINT, width: 8, height: 8, depth: 1, mips: 4, dimension: 4, union: [1, 1, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 1, first_layer: 0, probes: &[[0, 0, 0, 0], [3, 3, 0, 0], [2, 1, 0, 0]] },
        Case { name: "2D array", resource: 3, format: R8G8B8A8_UINT, width: 8, height: 8, depth: 4, mips: 3, dimension: 5, union: [0, 3, 0, 4, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 0, probes: &[[0, 0, 0, 0], [3, 2, 1, 0], [7, 5, 3, 0], [1, 1, 2, 1], [3, 3, 3, 1], [0, 1, 0, 2]] },
        Case { name: "2D array slices 1..3 from mip 1", resource: 3, format: R32G32B32A32_UINT, width: 8, height: 8, depth: 4, mips: 3, dimension: 5, union: [1, 2, 1, 2, 0, 0], mapping: DEFAULT, first_mip: 1, first_layer: 1, probes: &[[0, 0, 0, 0], [3, 2, 1, 0], [1, 1, 0, 1], [0, 0, 1, 1]] },
        Case { name: "3D", resource: 4, format: R8G8B8A8_UINT, width: 8, height: 8, depth: 8, mips: 4, dimension: 8, union: [0, 4, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 0, probes: &[[0, 0, 0, 0], [3, 2, 1, 0], [7, 7, 7, 0], [1, 1, 3, 1], [3, 0, 2, 1], [1, 1, 1, 2], [0, 0, 0, 3]] },
        Case { name: "3D from mip 1", resource: 4, format: R16G16_UINT, width: 8, height: 8, depth: 8, mips: 4, dimension: 8, union: [1, 3, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 1, first_layer: 0, probes: &[[0, 0, 0, 0], [3, 3, 3, 0], [1, 0, 1, 1]] },
        Case { name: "2D swizzled (B, G, R, A)", resource: 3, format: R8G8B8A8_UINT, width: 8, height: 8, depth: 1, mips: 1, dimension: 4, union: [0, 1, 0, 0, 0, 0], mapping: mapping(2, 1, 0, 3), first_mip: 0, first_layer: 0, probes: &[[0, 0, 0, 0], [5, 2, 0, 0], [7, 7, 0, 0]] },
        Case { name: "2D forced 0, 1 and A to R", resource: 3, format: R8G8B8A8_UINT, width: 8, height: 8, depth: 1, mips: 1, dimension: 4, union: [0, 1, 0, 0, 0, 0], mapping: mapping(4, 5, 3, 0), first_mip: 0, first_layer: 0, probes: &[[0, 0, 0, 0], [4, 4, 0, 0]] },
        Case { name: "2D one channel, the others default", resource: 3, format: R8_UINT, width: 8, height: 8, depth: 1, mips: 1, dimension: 4, union: [0, 1, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 0, probes: &[[0, 0, 0, 0], [6, 3, 0, 0]] },
        Case { name: "2D one channel read as RRRR", resource: 3, format: R32_UINT, width: 8, height: 8, depth: 1, mips: 1, dimension: 4, union: [0, 1, 0, 0, 0, 0], mapping: mapping(0, 0, 0, 0), first_mip: 0, first_layer: 0, probes: &[[0, 0, 0, 0], [6, 3, 0, 0]] },
        Case { name: "2D two channels", resource: 3, format: R16G16_UINT, width: 8, height: 8, depth: 1, mips: 1, dimension: 4, union: [0, 1, 0, 0, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 0, probes: &[[0, 0, 0, 0], [6, 3, 0, 0]] },
        Case { name: "2D array two slices", resource: 3, format: R16G16_UINT, width: 4, height: 4, depth: 5, mips: 2, dimension: 5, union: [0, 2, 2, 2, 0, 0], mapping: DEFAULT, first_mip: 0, first_layer: 2, probes: &[[0, 0, 0, 0], [3, 3, 1, 0], [1, 1, 1, 1]] },
    ]
}

/// (x, y, layer in the view, mip in the view) -> the texel's bytes as the data was filled.
fn source(c: &Case, probe: [u32; 4]) -> Vec<u8> {
    let [x, y, layer, mip] = probe;
    (0..c.format.channels * c.format.bytes).map(|k| texel_byte(c.first_mip + mip, c.first_layer + layer, y, x, k)).collect()
}

/// The four words a shader reads for `probe` through the case's mapping.
fn expected(c: &Case, texel: &[u8]) -> [u32; 4] {
    let f = c.format;
    let channel = |i: u32| -> u32 {
        if i >= f.channels {
            return if i == 3 { 1 } else { 0 };
        }
        let b = &texel[(i * f.bytes) as usize..][..f.bytes as usize];
        let v = b.iter().rev().fold(0u32, |a, &x| a << 8 | u32::from(x));
        match f.kind {
            Kind::Uint => v,
            Kind::Unorm => (v as f32 / 255.0).to_bits(),
        }
    };
    let one = match f.kind {
        Kind::Uint => 1,
        Kind::Unorm => 1.0f32.to_bits(),
    };
    let pick = |sel: u32| match sel {
        0..=3 => {
            let v = channel(sel);
            // a missing alpha is 1 in the format's own kind
            if sel >= f.channels && sel == 3 { one } else { v }
        }
        4 => 0,
        _ => one,
    };
    let m = c.mapping;
    [pick(m & 7), pick(m >> 3 & 7), pick(m >> 6 & 7), pick(m >> 9 & 7)]
}

fn hlsl_for(c: &Case) -> String {
    let t = if c.format.kind == Kind::Uint { "uint4" } else { "float4" };
    let (tex, load) = match c.dimension {
        2 => ("Texture1D", "tex.Load(int2(p.x, p.w))"),
        3 => ("Texture1DArray", "tex.Load(int3(p.x, p.z, p.w))"),
        4 => ("Texture2D", "tex.Load(int3(p.x, p.y, p.w))"),
        5 => ("Texture2DArray", "tex.Load(int4(p.x, p.y, p.z, p.w))"),
        8 => ("Texture3D", "tex.Load(int4(p.x, p.y, p.z, p.w))"),
        d => panic!("dimension {d}"),
    };
    let probes: Vec<String> = c.probes.iter().map(|p| format!("int4({}, {}, {}, {})", p[0], p[1], p[2], p[3])).collect();
    format!(
        r#"#define RS "DescriptorTable(SRV(t0, numDescriptors = 1)), UAV(u0)"
{tex}<{t}> tex : register(t0);
RWByteAddressBuffer o : register(u0);
static const int4 P[{n}] = {{ {list} }};
[RootSignature(RS)]
[numthreads({n}, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {{
    int4 p = P[id.x];
    {t} v = {load};
    {store}
}}
"#,
        n = probes.len(),
        list = probes.join(", "),
        store = if c.format.kind == Kind::Uint { "o.Store4(id.x * 16, v);" } else { "o.Store4(id.x * 16, asuint(v));" },
    )
}

/// Makes the console texture, fills the host copy, makes the view with `HostHeap::make_at` from the title's words.
fn console_texture(c: &Case) -> This {
    let (depth_or_array, _) = (c.depth, ());
    let words: [u32; 14] = [c.resource, 0, 0, 0, c.width, 0, c.height, depth_or_array | c.mips << 16, c.format.dxgi, 1, 0, 0, 0, 0];
    // SAFETY: fourteen words are a resource description (56 bytes, plain numbers), five a heap's properties.
    let (desc, props): (ResourceDesc, HeapProperties) = unsafe { (std::mem::transmute(words), std::mem::transmute([1u32, 0, 0, 0, 0])) };
    let mut out: This = std::ptr::null();
    let hr = crate::resource::create_committed(std::ptr::null(), &props, 0, &desc, 0, std::ptr::null(), &crate::resource::RESOURCE_IIDS[0], &mut out);
    assert_eq!(hr, S_OK, "{}: create_committed", c.name);
    out
}

fn fill(rig: &mut Rig, c: &Case, texture: &ID3D12Resource) {
    // SAFETY: a live texture.
    let desc = unsafe { texture.GetDesc() };
    let layers = if c.resource == 4 { 1 } else { c.depth };
    let mut data = Vec::new();
    for layer in 0..layers {
        for mip in 0..c.mips {
            let (w, h) = ((c.width >> mip).max(1), (c.height >> mip).max(1));
            let d = if c.resource == 4 { (c.depth >> mip).max(1) } else { 1 };
            let mut bytes = Vec::new();
            for z in 0..d {
                for y in 0..h {
                    for x in 0..w {
                        // the layer of a texel: its array slice, or its depth slice
                        let l = if c.resource == 4 { z } else { layer };
                        for k in 0..c.format.channels * c.format.bytes {
                            bytes.push(texel_byte(mip, l, y, x, k));
                        }
                    }
                }
            }
            data.push(bytes);
        }
    }
    rig.fill_texture(texture, &desc, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, &data);
}

fn view_words(c: &Case) -> [u32; 10] {
    let u = c.union;
    [c.format.dxgi, c.dimension, c.mapping, 0, u[0], u[1], u[2], u[3], u[4], u[5]]
}

#[test]
fn texture_views_read_the_texel_the_description_says() {
    let Some(mut rig) = Rig::new() else { return };
    let Some(heap) = HostHeap::new(false, 64) else { return };
    let mut failures = Vec::new();
    for c in cases() {
        let texture = console_texture(&c);
        let host_texture = crate::resource::host_resource(texture).expect("a host copy");
        fill(&mut rig, &c, host_texture);
        let slot = heap.dynamic(1).expect("a slot");
        heap.make_at(slot, &View::Shader(texture as usize, Some(view_words(&c))));
        let words = rig.compute(&hlsl_for(&c), &heap.heap, heap.gpu_handle(slot), 1, c.probes.len() * 4);
        for (i, p) in c.probes.iter().enumerate() {
            let want = expected(&c, &source(&c, *p));
            let got = &words[i * 4..i * 4 + 4];
            let same = |a: u32, b: u32| {
                if c.format.kind == Kind::Unorm { (f32::from_bits(a) - f32::from_bits(b)).abs() < 1.0 / 512.0 } else { a == b }
            };
            if !(0..4).all(|k| same(got[k], want[k])) {
                failures.push(format!("{}: probe {p:?}: read {got:x?}, the description says {want:x?}", c.name));
            }
        }
        heap.release(slot, 1);
        crate::object::slot::<extern "system" fn(This) -> u32>(texture, 0x10)(texture);
    }
    assert!(failures.is_empty(), "{} of the reads differ:\n{}", failures.len(), failures.join("\n"));
}

/// An unordered access view of one level (and slices) of a texture: a shader writes `value(x, y, z)` to the texels of
/// the view; the texture read back has them in the level and slices the description names, and nothing anywhere else.
struct Write {
    name: &'static str,
    resource: u32,
    width: u32,
    height: u32,
    depth: u32,
    mips: u32,
    /// `D3D12_UAV_DIMENSION`: 2 = 1D, 3 = 1D array, 4 = 2D, 5 = 2D array, 8 = 3D.
    dimension: u32,
    /// The words after the dimension.
    union: [u32; 4],
    /// The level, and the slices (array slices or depth slices) of the view.
    mip: u32,
    first: u32,
    count: u32,
}

fn uav_cases() -> Vec<Write> {
    vec![
        Write { name: "1D", resource: 2, width: 16, height: 1, depth: 1, mips: 3, dimension: 2, union: [1, 0, 0, 0], mip: 1, first: 0, count: 1 },
        Write { name: "1D array", resource: 2, width: 8, height: 1, depth: 4, mips: 2, dimension: 3, union: [0, 1, 2, 0], mip: 0, first: 1, count: 2 },
        Write { name: "2D level 1", resource: 3, width: 8, height: 8, depth: 1, mips: 3, dimension: 4, union: [1, 0, 0, 0], mip: 1, first: 0, count: 1 },
        Write { name: "2D level 0 of one", resource: 3, width: 8, height: 4, depth: 1, mips: 1, dimension: 4, union: [0, 0, 0, 0], mip: 0, first: 0, count: 1 },
        Write { name: "2D array slices 1 and 2 of 4, level 1", resource: 3, width: 8, height: 8, depth: 4, mips: 3, dimension: 5, union: [1, 1, 2, 0], mip: 1, first: 1, count: 2 },
        Write { name: "3D level 1, all depth slices", resource: 4, width: 8, height: 8, depth: 8, mips: 3, dimension: 8, union: [1, 0, 4, 0], mip: 1, first: 0, count: 4 },
        Write { name: "3D level 0, all", resource: 4, width: 4, height: 4, depth: 4, mips: 1, dimension: 8, union: [0, 0, 4, 0], mip: 0, first: 0, count: 4 },
    ]
}

fn value(x: u32, y: u32, z: u32) -> u32 {
    0x1000_0000 + z * 0x10_0000 + y * 0x1000 + x * 3 + 1
}

#[test]
fn unordered_access_views_write_the_level_and_slices_the_description_says() {
    let Some(mut rig) = Rig::new() else { return };
    let Some(heap) = HostHeap::new(false, 64) else { return };
    let mut failures = Vec::new();
    for c in uav_cases() {
        // console flags 4: allow unordered access
        let words: [u32; 14] = [c.resource, 0, 0, 0, c.width, 0, c.height, c.depth | c.mips << 16, 42, 1, 0, 0, 4, 0];
        // SAFETY: a resource description and heap properties, plain numbers.
        let (desc, props): (ResourceDesc, HeapProperties) = unsafe { (std::mem::transmute(words), std::mem::transmute([1u32, 0, 0, 0, 0])) };
        let mut texture: This = std::ptr::null();
        assert_eq!(crate::resource::create_committed(std::ptr::null(), &props, 0, &desc, 0, std::ptr::null(), &crate::resource::RESOURCE_IIDS[0], &mut texture), S_OK, "{}", c.name);
        let host_texture = crate::resource::host_resource(texture).expect("a host copy");
        // SAFETY: a live texture.
        let host_desc = unsafe { host_texture.GetDesc() };
        let layers = if c.resource == 4 { 1 } else { c.depth };
        let size = |mip: u32| ((c.width >> mip).max(1), (c.height >> mip).max(1), if c.resource == 4 { (c.depth >> mip).max(1) } else { 1 });
        let zeros: Vec<Vec<u8>> = (0..layers)
            .flat_map(|_| (0..c.mips).map(|m| {
                let (w, h, d) = size(m);
                vec![0u8; (w * h * d * 4) as usize]
            }))
            .collect();
        rig.fill_texture(host_texture, &host_desc, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, &zeros);
        let slot = heap.dynamic(1).expect("a slot");
        let w = [42, c.dimension, c.union[0], c.union[1], c.union[2], c.union[3], 0, 0, 0, 0];
        heap.make_at(slot, &View::Unordered(texture as usize, 0, Some(w)));
        let (vw, vh, _) = size(c.mip);
        let (target, index) = match c.dimension {
            2 => ("RWTexture1D<uint>", "x"),
            3 => ("RWTexture1DArray<uint>", "uint2(x, z)"),
            4 => ("RWTexture2D<uint>", "uint2(x, y)"),
            5 => ("RWTexture2DArray<uint>", "uint3(x, y, z)"),
            _ => ("RWTexture3D<uint>", "uint3(x, y, z)"),
        };
        let (h, d) = (if c.dimension == 2 || c.dimension == 3 { 1 } else { vh }, if matches!(c.dimension, 2 | 4) { 1 } else { c.count });
        let source = format!(
            r#"#define RS "DescriptorTable(UAV(u1, numDescriptors = 1)), UAV(u0)"
{target} t : register(u1);
RWByteAddressBuffer o : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {{
    for (uint z = 0; z < {d}; z++)
        for (uint y = 0; y < {h}; y++)
            for (uint x = 0; x < {vw}; x++)
                t[{index}] = 0x10000000 + z * 0x100000 + y * 0x1000 + x * 3 + 1;
    o.Store(0, 1);
}}
"#
        );
        rig.compute(&source, &heap.heap, heap.gpu_handle(slot), 1, 1);
        let back = rig.read_texture(host_texture, &host_desc, layers * c.mips, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        for layer in 0..layers {
            for mip in 0..c.mips {
                let (mw, mh, md) = size(mip);
                let bytes = &back[(layer * c.mips + mip) as usize];
                for z in 0..md {
                    for y in 0..mh {
                        for x in 0..mw {
                            let got = u32::from_le_bytes(bytes[((z * mh + y) * mw + x) as usize * 4..][..4].try_into().unwrap());
                            // the slice of the view this texel is in, if any: depth slices of a 3D texture, array slices
                            // for the array views, the one slice of a view of no array
                            let slice = if c.resource == 4 { z } else { layer };
                            let sliced = matches!(c.dimension, 3 | 5 | 8);
                            let inside = mip == c.mip && if sliced { (c.first..c.first + c.count).contains(&slice) } else { slice == 0 };
                            let want = if inside { value(x, y, if sliced { slice - c.first } else { 0 }) } else { 0 };
                            if got != want && failures.len() < 12 {
                                failures.push(format!("{}: layer {layer} level {mip} texel ({x},{y},{z}): {got:#x}, the description says {want:#x}", c.name));
                            }
                        }
                    }
                }
            }
        }
        heap.release(slot, 1);
        crate::object::slot::<extern "system" fn(This) -> u32>(texture, 0x10)(texture);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A null view reads zeros: a null texture description of any dimension makes a descriptor the shader can read.
#[test]
fn null_shader_views_read_zeros() {
    let Some(mut rig) = Rig::new() else { return };
    let Some(heap) = HostHeap::new(false, 64) else { return };
    for (name, dimension, tex, load, mapping) in [
        ("2D", 4u32, "Texture2D<float4>", "tex.Load(int3(1, 1, 0))", DEFAULT),
        ("2D array", 5, "Texture2DArray<float4>", "tex.Load(int4(1, 1, 1, 0))", DEFAULT),
        ("1D", 2, "Texture1D<float4>", "tex.Load(int2(1, 0))", DEFAULT),
        ("3D", 8, "Texture3D<float4>", "tex.Load(int4(1, 1, 1, 0))", DEFAULT),
        ("cube", 9, "TextureCube<float4>", "tex.SampleLevel(smp, float3(1, 0, 0), 0)", DEFAULT),
    ] {
        let slot = heap.dynamic(1).expect("a slot");
        let words = [28, dimension, mapping, 0, 0, 1, 0, 0, 0, 0];
        heap.make_at(slot, &View::Shader(0, Some(words)));
        let source = format!(
            r#"#define RS "DescriptorTable(SRV(t0, numDescriptors = 1)), UAV(u0), StaticSampler(s0, filter = FILTER_MIN_MAG_MIP_POINT)"
{tex} tex : register(t0);
SamplerState smp : register(s0);
RWByteAddressBuffer o : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {{
    float4 v = {load};
    o.Store4(0, asuint(v));
}}
"#
        );
        let words = rig.compute(&source, &heap.heap, heap.gpu_handle(slot), 1, 4);
        assert_eq!(words, [0, 0, 0, 0], "a null {name} view");
        heap.release(slot, 1);
    }
}

/// A 3D unordered access view of some of the depth slices (`FirstWSlice`, `WSize`): the D3D12 spec has it write
/// those slices only. D3DMetal says "Creating a partial view of a 3D texture is not supported!" and gives the
/// whole level instead, so this test reports what the host does and fails only on the Metal back, which has
/// no such limit to inherit (matrix row C-V-012, finding F1). Set `VICTORY_PROVE_PARTIAL_3D=strict` to make it fail on
/// any host that does not honour the slices.
#[test]
fn partial_3d_unordered_access_views_write_only_their_slices() {
    let Some(mut rig) = Rig::new() else { return };
    let Some(heap) = HostHeap::new(false, 64) else { return };
    let words: [u32; 14] = [4, 0, 0, 0, 4, 0, 4, 4 | 1 << 16, 42, 1, 0, 0, 4, 0];
    // SAFETY: a resource description and heap properties, plain numbers.
    let (desc, props): (ResourceDesc, HeapProperties) = unsafe { (std::mem::transmute(words), std::mem::transmute([1u32, 0, 0, 0, 0])) };
    let mut texture: This = std::ptr::null();
    assert_eq!(crate::resource::create_committed(std::ptr::null(), &props, 0, &desc, 0, std::ptr::null(), &crate::resource::RESOURCE_IIDS[0], &mut texture), S_OK);
    let host_texture = crate::resource::host_resource(texture).expect("a host copy");
    // SAFETY: a live texture.
    let host_desc = unsafe { host_texture.GetDesc() };
    rig.fill_texture(host_texture, &host_desc, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, &[vec![0u8; 4 * 4 * 4 * 4]]);
    let slot = heap.dynamic(1).expect("a slot");
    // slices 1 and 2 of 4
    heap.make_at(slot, &View::Unordered(texture as usize, 0, Some([42, 8, 0, 1, 2, 0, 0, 0, 0, 0])));
    let source = r#"#define RS "DescriptorTable(UAV(u1, numDescriptors = 1)), UAV(u0)"
RWTexture3D<uint> t : register(u1);
RWByteAddressBuffer o : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {
    for (uint z = 0; z < 2; z++)
        for (uint y = 0; y < 4; y++)
            for (uint x = 0; x < 4; x++)
                t[uint3(x, y, z)] = 0xabc00000 + z;
    o.Store(0, 1);
}
"#;
    rig.compute(source, &heap.heap, heap.gpu_handle(slot), 1, 1);
    let back = rig.read_texture(host_texture, &host_desc, 1, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    let word = |x: usize, y: usize, z: usize| u32::from_le_bytes(back[0][((z * 4 + y) * 4 + x) * 4..][..4].try_into().unwrap());
    let honoured = (word(0, 0, 0), word(0, 0, 1), word(0, 0, 2), word(0, 0, 3)) == (0, 0xabc0_0000, 0xabc0_0001, 0);
    eprintln!("partial 3D unordered access view: depth slices 0..3 read {:#x} {:#x} {:#x} {:#x}; slices honoured: {honoured}", word(0, 0, 0), word(0, 0, 1), word(0, 0, 2), word(0, 0, 3));
    if std::env::var("VICTORY_PROVE_PARTIAL_3D").is_ok_and(|v| v == "strict") {
        assert!(honoured, "the host does not honour FirstWSlice / WSize of a 3D unordered access view");
    }
    heap.release(slot, 1);
}

/// Cube views: face `f` of cube `c` is array slice `6 c + f` (the spec's cube face order +X, -X, +Y, -Y, +Z, -Z); a view
/// of `First2DArrayFace` and `NumCubes` shows the cubes from there. Point sampling at a face's centre reads its centre texel.
#[test]
fn cube_views_show_the_faces_in_the_specs_order() {
    let Some(mut rig) = Rig::new() else { return };
    let Some(heap) = HostHeap::new(false, 64) else { return };
    const DIRECTIONS: [&str; 6] = ["float3(1, 0, 0)", "float3(-1, 0, 0)", "float3(0, 1, 0)", "float3(0, -1, 0)", "float3(0, 0, 1)", "float3(0, 0, -1)"];
    // (name, resource array size, dimension, union after the pad: most detailed mip, levels, first face, cubes)
    for (name, array, dimension, union, first_mip, first_face, cubes) in [
        ("cube", 6u32, 9u32, [0u32, 2, 0, 0, 0, 0], 0u32, 0u32, 1u32),
        ("cube from mip 1", 6, 9, [1, 1, 0, 0, 0, 0], 1, 0, 1),
        ("cube array of 2", 12, 10, [0, 2, 0, 2, 0, 0], 0, 0, 2),
        ("cube array: the second of 2 (first face 6)", 12, 10, [0, 2, 6, 1, 0, 0], 0, 6, 1),
    ] {
        let case = Case { name, resource: 3, format: R8G8B8A8_UNORM, width: 3, height: 3, depth: array, mips: 2, dimension, union, mapping: DEFAULT, first_mip, first_layer: first_face, probes: &[] };
        let texture = console_texture(&case);
        let host_texture = crate::resource::host_resource(texture).expect("a host copy");
        fill(&mut rig, &case, host_texture);
        let slot = heap.dynamic(1).expect("a slot");
        heap.make_at(slot, &View::Shader(texture as usize, Some(view_words(&case))));
        let levels = if first_mip == 1 { 1 } else { 2 };
        let mut probes = Vec::new();
        for cube in 0..cubes {
            for face in 0..6u32 {
                for mip in 0..levels {
                    probes.push((cube, face, mip));
                }
            }
        }
        let calls: Vec<String> = probes
            .iter()
            .map(|&(cube, face, mip)| {
                let d = DIRECTIONS[face as usize];
                if dimension == 9 { format!("cube.SampleLevel(smp, {d}, {mip})") } else { format!("cube.SampleLevel(smp, float4({d}, {cube}), {mip})") }
            })
            .collect();
        let type_name = if dimension == 9 { "TextureCube<float4>" } else { "TextureCubeArray<float4>" };
        let body: String = calls.iter().enumerate().map(|(i, c)| format!("    o.Store4({}, asuint({c}));\n", i * 16)).collect();
        let source = format!(
            r#"#define RS "DescriptorTable(SRV(t0, numDescriptors = 1)), UAV(u0), StaticSampler(s0, filter = FILTER_MIN_MAG_MIP_POINT)"
{type_name} cube : register(t0);
SamplerState smp : register(s0);
RWByteAddressBuffer o : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {{
{body}}}
"#
        );
        let words = rig.compute(&source, &heap.heap, heap.gpu_handle(slot), 1, probes.len() * 4);
        for (i, &(cube, face, mip)) in probes.iter().enumerate() {
            let m = first_mip + mip;
            let side = (3u32 >> m).max(1);
            let centre = side / 2;
            let layer = first_face + 6 * cube + face;
            for k in 0..4 {
                let want = f32::from(texel_byte(m, layer, centre, centre, k)) / 255.0;
                let got = f32::from_bits(words[i * 4 + k as usize]);
                assert!((got - want).abs() < 1.0 / 512.0, "{name}: cube {cube} face {face} level {mip}: component {k} reads {got}, array slice {layer} level {m} has {want}");
            }
        }
        heap.release(slot, 1);
        crate::object::slot::<extern "system" fn(This) -> u32>(texture, 0x10)(texture);
    }
}

/// Constant buffer views (api.CreateConstantBufferView, 30,905 calls in the 370 s run): the view's size is rounded up
/// to 256 bytes and held at 64 KiB (D3D12 `D3D12_CONSTANT_BUFFER_VIEW_DESC`: `SizeInBytes` a multiple of 256 and at most
/// 65536 constants of 16 bytes), `BufferLocation` a multiple of 256 (else the view is not made: the slot reads zeros),
/// and a shader reads the 16-byte constants from the location on.
#[test]
fn constant_buffer_views_read_the_constants_at_their_location() {
    assert_eq!([0u32, 1, 255, 256, 257, 480, 65_536, 65_537, u32::MAX].map(constant_bytes), [0, 256, 256, 256, 512, 512, 65_536, 65_536, 65_536]);
    let Some(mut rig) = Rig::new() else { return };
    let Some(heap) = HostHeap::new(false, 64) else { return };
    let (upload, at) = crate::host::host().unwrap().upload_buffer(1 << 16).expect("an upload buffer");
    // 8 constants at offset 256 of the buffer, then others that must not be read
    let words: Vec<u32> = (0..64u32).map(|i| 0xc0de_0000 + i).collect();
    // SAFETY: the mapped upload buffer has 64 KiB.
    unsafe {
        std::ptr::write_bytes(at, 0, 1 << 16);
        std::ptr::copy_nonoverlapping(words.as_ptr().cast::<u8>(), at.add(256), 256);
    }
    let slot = heap.dynamic(1).expect("a slot");
    // SAFETY: a live upload buffer.
    let gpu = unsafe { upload.GetGPUVirtualAddress() } + 256;
    heap.constant_at(slot, &upload, gpu, 128);
    let source = r#"#define RS "DescriptorTable(CBV(b0, numDescriptors = 1)), UAV(u0)"
cbuffer C : register(b0) { uint4 c[8]; };
RWByteAddressBuffer o : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {
    for (uint i = 0; i < 8; i++) o.Store4(i * 16, c[i]);
}
"#;
    let out = rig.compute(source, &heap.heap, heap.gpu_handle(slot), 1, 32);
    assert_eq!(&out[..32], &words[..32], "eight 16-byte constants from the location");
    heap.release(slot, 1);
}
