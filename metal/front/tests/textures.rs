//! Textures, heaps, views and copies in the core, against the mock back:
//! what the front sends for what the application does.

use std::sync::Arc;

use victory_metal_front::descriptor::{self, HeapDesc};
use victory_metal_front::device::*;
use victory_metal_front::hresult::Hr;
use victory_metal_front::mock::{Descriptor, MockLink};
use victory_metal_front::resource::{self, HeapProps, ResourceDesc};
use victory_metal_front::state;
use victory_metal_front::view::{self, TargetView, TextureView};
use victory_metal_proto::cmd::*;
use victory_metal_proto::sync::SyncOp;
use victory_metal_proto::types::{TexViewDim, ViewKind, swizzle};
use victory_metal_proto::{Command, Decoder, ResId};

fn setup() -> (Arc<MockLink>, Arc<Device>) {
    let link = Arc::new(MockLink::new());
    let sys = System::open(link.clone()).unwrap();
    (link, Device::new(sys))
}

fn tex2d(w: u64, h: u32, array: u16, mips: u16, format: u32, flags: u32) -> ResourceDesc {
    ResourceDesc {
        dimension: resource::DIM_TEXTURE2D,
        alignment: 0,
        width: w,
        height: h,
        depth_or_array: array,
        mips,
        format,
        sample_count: 1,
        sample_quality: 0,
        layout: resource::LAYOUT_UNKNOWN,
        flags,
    }
}

const RGBA8: u32 = 28;
const RGBA8_TYPELESS: u32 = 27;

fn default_props() -> HeapProps {
    HeapProps::of_type(resource::HEAP_DEFAULT)
}

fn custom(cpu_page: u32, pool: u32) -> HeapProps {
    HeapProps {
        ty: resource::HEAP_CUSTOM,
        cpu_page,
        pool,
    }
}

fn commands(link: &MockLink) -> Vec<Command<'static>> {
    let mut out = Vec::new();
    for b in link.blocks() {
        for c in Decoder::new(&b).unwrap().collect_all().unwrap() {
            out.push(into_static(c));
        }
    }
    out
}

fn into_static(c: Command<'_>) -> Command<'static> {
    // Texture commands carry no borrowed data.
    match c {
        Command::CopyTextureRegion(x) => Command::CopyTextureRegion(x),
        Command::CopyResource(x) => Command::CopyResource(x),
        Command::ClearUavTexture(x) => Command::ClearUavTexture(x),
        Command::BeginList(x) => Command::BeginList(x),
        Command::EndList(x) => Command::EndList(x),
        Command::Barrier(x) => Command::Barrier(x),
        Command::CopyBufferRegion(x) => Command::CopyBufferRegion(x),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn committed_textures_reach_the_back_with_their_usage_and_are_destroyed() {
    let (link, dev) = setup();
    let t = dev
        .create_committed(
            default_props(),
            &tex2d(
                64,
                32,
                1,
                0,
                RGBA8_TYPELESS,
                resource::FLAG_ALLOW_UNORDERED_ACCESS | resource::FLAG_ALLOW_RENDER_TARGET,
            ),
            state::COMMON,
        )
        .unwrap();
    assert!(t.is_texture());
    assert_eq!(t.desc.mips, 7, "0 means the whole chain");
    assert_eq!(t.gpu_va, 0);
    assert_eq!(t.map(0), Err(Hr::E_INVALIDARG));
    let rec = link.texture(t.res).unwrap();
    assert_eq!(
        (rec.width, rec.height, rec.mips, rec.format),
        (64, 32, 7, RGBA8_TYPELESS)
    );
    use victory_metal_proto::types::tex_usage::*;
    assert_eq!(rec.usage, SRV | UAV | RENDER_TARGET);
    assert_eq!(rec.placed, None);
    // Depth without shader access.
    let d = dev
        .create_committed(
            default_props(),
            &tex2d(
                16,
                16,
                1,
                1,
                45,
                resource::FLAG_ALLOW_DEPTH_STENCIL | resource::FLAG_DENY_SHADER_RESOURCE,
            ),
            state::COMMON,
        )
        .unwrap();
    assert_eq!(link.texture(d.res).unwrap().usage, DEPTH_STENCIL);
    assert_eq!(link.live_textures(), 2);
    drop(t);
    drop(d);
    assert_eq!(link.live_textures(), 0);
    // Bad descriptions never reach the back.
    let before = link.calls_of(SyncOp::CREATE_TEXTURE);
    let mut bad = tex2d(8, 8, 1, 1, 0, 0);
    assert_eq!(
        dev.create_committed(default_props(), &bad, 0).err(),
        Some(Hr::E_INVALIDARG),
        "format UNKNOWN"
    );
    bad = tex2d(8, 8, 1, 1, RGBA8, 0);
    assert_eq!(
        dev.create_committed(
            HeapProps::of_type(resource::HEAP_UPLOAD),
            &bad,
            state::GENERIC_READ
        )
        .err(),
        Some(Hr::E_INVALIDARG),
        "textures do not live in upload heaps"
    );
    bad.layout = resource::LAYOUT_ROW_MAJOR;
    assert_eq!(
        dev.create_committed(default_props(), &bad, 0).err(),
        Some(Hr::E_NOTIMPL)
    );
    assert_eq!(link.calls_of(SyncOp::CREATE_TEXTURE), before);
}

#[test]
fn the_subresource_state_table_covers_every_mip_slice_and_plane() {
    let (_link, dev) = setup();
    // Depth-stencil: 2 planes x 3 mips x 4 slices.
    let t = dev
        .create_committed(
            default_props(),
            &tex2d(16, 16, 4, 3, 45, resource::FLAG_ALLOW_DEPTH_STENCIL),
            state::COMMON,
        )
        .unwrap();
    assert_eq!(t.desc.subresource_count(), 24);
    let mut l = CommandList::new(&dev, 0);
    l.resource_barrier(&[ResourceBarrier::Transition {
        res: t.res,
        subresource: 23,
        before: state::COMMON,
        after: state::DEPTH_WRITE,
        flags: 0,
    }]);
    l.resource_barrier(&[ResourceBarrier::Transition {
        res: t.res,
        subresource: 24,
        before: state::COMMON,
        after: state::DEPTH_WRITE,
        flags: 0,
    }]);
    assert_eq!(
        l.close().err(),
        Some(Hr::E_INVALIDARG),
        "subresource 24 does not exist"
    );
}

#[test]
fn custom_heaps_map_to_cpu_visible_or_private_memory() {
    let (link, dev) = setup();
    let size = 4096;
    let desc = ResourceDesc::buffer(size, 0);
    for page in [
        resource::CPU_PAGE_WRITE_BACK,
        resource::CPU_PAGE_WRITE_COMBINE,
    ] {
        for pool in [resource::POOL_L0, resource::POOL_L1] {
            let b = dev
                .create_committed(custom(page, pool), &desc, state::COMMON)
                .unwrap();
            assert_ne!(b.cpu_addr, 0, "page {page} pool {pool}");
            assert_eq!(b.map(0), Ok(b.cpu_addr));
            assert_eq!(b.props.ty, resource::HEAP_CUSTOM);
        }
    }
    let g = dev
        .create_committed(
            custom(resource::CPU_PAGE_NOT_AVAILABLE, resource::POOL_L1),
            &desc,
            0,
        )
        .unwrap();
    assert_eq!(g.cpu_addr, 0);
    assert_eq!(g.map(0), Err(Hr::E_INVALIDARG));
    // A custom heap that says nothing is refused, as the API does.
    assert_eq!(
        dev.create_committed(
            custom(resource::CPU_PAGE_UNKNOWN, resource::POOL_L0),
            &desc,
            0
        )
        .err(),
        Some(Hr::E_INVALIDARG)
    );
    // Textures in custom CPU-visible memory are private textures.
    let t = dev
        .create_committed(
            custom(resource::CPU_PAGE_WRITE_BACK, resource::POOL_L0),
            &tex2d(8, 8, 1, 1, RGBA8, 0),
            0,
        )
        .unwrap();
    assert!(link.texture(t.res).is_some());
    assert_eq!(t.map(0), Err(Hr::E_INVALIDARG));
    // Custom heaps hold any initial state.
    assert!(
        dev.create_committed(
            custom(resource::CPU_PAGE_WRITE_BACK, resource::POOL_L0),
            &desc,
            state::COPY_DEST
        )
        .is_ok()
    );
}

#[test]
fn heaps_place_buffers_and_textures_and_keep_themselves_alive() {
    let (link, dev) = setup();
    assert_eq!(
        dev.create_heap(default_props(), 0, 0, 0).err(),
        Some(Hr::E_INVALIDARG)
    );
    assert_eq!(
        dev.create_heap(default_props(), 65536 + 1, 0, 0).err(),
        Some(Hr::E_INVALIDARG),
        "a multiple of the alignment"
    );
    assert_eq!(
        dev.create_heap(default_props(), 65536, 4096, 0).err(),
        Some(Hr::E_INVALIDARG)
    );
    let heap = Arc::new(dev.create_heap(default_props(), 1 << 20, 0, 0).unwrap());
    assert_eq!(heap.alignment, 65536);
    assert_eq!(link.live_mem_heaps(), 1);
    let t = dev
        .create_placed(&heap, 65536, &tex2d(64, 64, 1, 1, RGBA8, 0), state::COMMON)
        .unwrap();
    assert_eq!(
        link.texture(t.res).unwrap().placed,
        Some((heap.id.0, 65536))
    );
    let b = dev
        .create_placed(
            &heap,
            0,
            &ResourceDesc::buffer(4096, resource::FLAG_ALLOW_UNORDERED_ACCESS),
            state::COMMON,
        )
        .unwrap();
    assert_ne!(b.gpu_va, 0);
    // Alignment and range.
    assert_eq!(
        dev.create_placed(&heap, 100, &tex2d(8, 8, 1, 1, RGBA8, 0), 0)
            .err(),
        Some(Hr::E_INVALIDARG)
    );
    assert_eq!(
        dev.create_placed(&heap, 1 << 20, &tex2d(8, 8, 1, 1, RGBA8, 0), 0)
            .err(),
        Some(Hr::E_INVALIDARG)
    );
    // The application drops its heap reference; the resources keep it.
    drop(heap);
    assert_eq!(link.live_mem_heaps(), 1);
    drop(t);
    assert_eq!(link.live_mem_heaps(), 1);
    drop(b);
    assert_eq!(link.live_mem_heaps(), 0);

    // A custom heap with CPU access places buffers with a CPU pointer.
    let h = Arc::new(
        dev.create_heap(
            custom(resource::CPU_PAGE_WRITE_BACK, resource::POOL_L0),
            1 << 20,
            0,
            0,
        )
        .unwrap(),
    );
    let cb = dev
        .create_placed(&h, 65536, &ResourceDesc::buffer(256, 0), state::COMMON)
        .unwrap();
    assert_ne!(cb.cpu_addr, 0);
    assert_eq!(h.props.ty, resource::HEAP_CUSTOM);
}

#[test]
fn allocation_info_adds_up_placements() {
    let (_link, dev) = setup();
    let t = tex2d(256, 256, 1, 1, RGBA8, 0);
    let (one, a1) = dev.allocation_info(&[t]).unwrap();
    assert_eq!(a1, 65536);
    assert!(one >= 256 * 256 * 4 && one % 65536 == 0);
    let (two, _) = dev.allocation_info(&[t, t]).unwrap();
    assert_eq!(two, 2 * one);
    let (buf, ab) = dev
        .allocation_info(&[ResourceDesc::buffer(100, 0)])
        .unwrap();
    assert_eq!((buf, ab), (65536, 65536));
    // MSAA aligns to 4 MiB.
    let mut ms = t;
    ms.sample_count = 4;
    assert_eq!(dev.allocation_info(&[ms]).unwrap().1, 4 << 20);
    // An invalid description has no size.
    assert_eq!(dev.allocation_info(&[tex2d(8, 8, 1, 1, 0, 0)]), None);
}

fn view_heap(dev: &Arc<Device>, ty: u32, count: u32, visible: bool) -> DescriptorHeap {
    dev.create_descriptor_heap(&HeapDesc {
        ty,
        count,
        flags: if visible {
            descriptor::FLAG_SHADER_VISIBLE
        } else {
            0
        },
        node_mask: 0,
    })
    .unwrap()
}

#[test]
fn texture_views_reach_the_back_and_uavs_are_remembered() {
    let (link, dev) = setup();
    let inc = dev.descriptor_increment(descriptor::TYPE_CBV_SRV_UAV) as u64;
    let heap = view_heap(&dev, descriptor::TYPE_CBV_SRV_UAV, 8, true);
    let cpu = |i: u64| heap.cpu_start() + i * inc;
    let gpu = |i: u64| heap.gpu_start() + i * inc;
    let t = dev
        .create_committed(
            default_props(),
            &tex2d(
                32,
                32,
                4,
                5,
                RGBA8_TYPELESS,
                resource::FLAG_ALLOW_UNORDERED_ACCESS,
            ),
            state::COMMON,
        )
        .unwrap();
    // SRV: sRGB reinterpretation of mips 1.., all slices.
    let mut srv = view::default_view(&t.desc, t.res, false);
    srv.format = 29;
    srv.mip_first = 1;
    srv.min_lod = 0.5;
    dev.create_texture_view(cpu(0), &srv).unwrap();
    let Some(Descriptor::View(v)) = link.descriptor(heap.back, 0) else {
        panic!()
    };
    assert_eq!(v.kind, ViewKind::TEXTURE_SRV);
    assert_eq!(
        (v.resource, v.format, v.dimension),
        (t.res, 29, TexViewDim::TEX2D_ARRAY)
    );
    assert_eq!((v.mip_first, v.mip_count, v.slice_count), (1, 0, 0));
    assert_eq!(v.swizzle, swizzle::IDENTITY);
    assert_eq!(v.min_lod, 0.5);
    // UAV of one slice of mip 2; remembered, and cleared through the CPU handle.
    let mut uav = view::default_view(&t.desc, t.res, true);
    uav.format = 33;
    uav.dim = TexViewDim::TEX2D;
    uav.mip_first = 2;
    uav.slice_first = 3;
    uav.slice_count = 1;
    dev.create_texture_view(cpu(1), &uav).unwrap();
    assert_eq!(dev.texture_uav(cpu(1), 0), Some(uav));
    // The descriptor copied to a shader-visible slot is found by its GPU handle.
    dev.copy_descriptors(cpu(5), cpu(1), 1).unwrap();
    assert_eq!(dev.texture_uav(0, gpu(5)), Some(uav));
    assert_eq!(dev.texture_uav(cpu(2), gpu(2)), None);
    // Overwriting with a buffer view forgets it.
    dev.create_cbv(cpu(5), 0, 0).unwrap();
    assert_eq!(dev.texture_uav(0, gpu(5)), None);
    // A null texture view.
    let mut null = uav;
    null.resource = None;
    dev.create_texture_view(cpu(1), &null).unwrap();
    assert_eq!(
        link.descriptor(heap.back, 1),
        Some(Descriptor::View(Default::default()))
    );
    assert_eq!(dev.texture_uav(cpu(1), 0), None);
}

#[test]
fn render_target_and_depth_views_are_records_that_copy() {
    let (link, dev) = setup();
    let rtv = view_heap(&dev, descriptor::TYPE_RTV, 4, false);
    let dsv = view_heap(&dev, descriptor::TYPE_DSV, 2, false);
    assert_eq!(rtv.back.0, 0, "no memory on the back");
    let inc = dev.descriptor_increment(descriptor::TYPE_RTV) as u64;
    let t = dev
        .create_committed(
            default_props(),
            &tex2d(
                16,
                16,
                6,
                3,
                RGBA8_TYPELESS,
                resource::FLAG_ALLOW_RENDER_TARGET,
            ),
            state::COMMON,
        )
        .unwrap();
    let tv = TargetView {
        res: t.res,
        format: RGBA8,
        dim: TexViewDim::TEX2D_ARRAY,
        depth_stencil: false,
        mip: 1,
        first_slice: 2,
        slice_count: 3,
        plane: 0,
        read_only: 0,
    };
    dev.create_target_view(rtv.cpu_start() + inc, Some(tv))
        .unwrap();
    assert_eq!(dev.target_view(rtv.cpu_start() + inc), Some(tv));
    assert_eq!(dev.target_view(rtv.cpu_start()), None);
    // CopyDescriptorsSimple within and between unbacked heaps carries the record.
    dev.copy_descriptors(rtv.cpu_start() + 3 * inc, rtv.cpu_start() + inc, 1)
        .unwrap();
    assert_eq!(dev.target_view(rtv.cpu_start() + 3 * inc), Some(tv));
    dev.copy_descriptors(dsv.cpu_start(), rtv.cpu_start() + inc, 1)
        .unwrap();
    assert_eq!(dev.target_view(dsv.cpu_start()), Some(tv));
    // Between a heap the back has and one it does not: refused.
    let shader = view_heap(&dev, descriptor::TYPE_CBV_SRV_UAV, 2, true);
    assert_eq!(
        dev.copy_descriptors(shader.cpu_start(), rtv.cpu_start(), 1),
        Err(Hr::E_INVALIDARG)
    );
    // A null view clears the slot.
    dev.create_target_view(rtv.cpu_start() + inc, None).unwrap();
    assert_eq!(dev.target_view(rtv.cpu_start() + inc), None);
    assert_eq!(link.calls_of(SyncOp::CREATE_DESC_HEAP), 1);
    // The record dies with its heap.
    let h = dsv.cpu_start();
    drop(dsv);
    assert_eq!(dev.target_view(h), None);
}

#[test]
fn texture_copies_encode_locations_boxes_and_clears() {
    let (link, dev) = setup();
    let up = dev
        .create_committed(
            HeapProps::of_type(resource::HEAP_UPLOAD),
            &ResourceDesc::buffer(1 << 16, 0),
            state::GENERIC_READ,
        )
        .unwrap();
    let t = dev
        .create_committed(
            default_props(),
            &tex2d(16, 16, 3, 2, RGBA8, resource::FLAG_ALLOW_UNORDERED_ACCESS),
            state::COMMON,
        )
        .unwrap();
    let t2 = dev
        .create_committed(
            default_props(),
            &tex2d(16, 16, 3, 2, RGBA8, 0),
            state::COMMON,
        )
        .unwrap();
    let odd = dev
        .create_committed(default_props(), &tex2d(8, 8, 3, 2, RGBA8, 0), state::COMMON)
        .unwrap();
    let foot = CopyLoc::Footprint {
        res: &up,
        offset: 512,
        format: RGBA8,
        width: 16,
        height: 16,
        depth: 1,
        row_pitch: 256,
    };
    let mut l = CommandList::new(&dev, 0);
    // Subresource 4 = mip 0 of slice 2 (mips = 2).
    l.copy_texture_region(
        &CopyLoc::Subresource { res: &t, index: 4 },
        [0; 3],
        &foot,
        None,
    );
    l.copy_texture_region(
        &CopyLoc::Subresource { res: &t2, index: 3 },
        [4, 5, 0],
        &CopyLoc::Subresource { res: &t, index: 5 },
        Some([1, 2, 0, 3, 4, 1]),
    );
    l.copy_resource(&t2, &t);
    l.clear_uav_texture(
        &t,
        &texture_uav_of(&t),
        [1, 2, 3, 4],
        false,
        &[[0, 0, 8, 8], [2, 2, 4, 4]],
    );
    let items = l.close().unwrap();
    assert!(link.rejected().is_empty());
    let q = dev.create_queue(0, 0).unwrap();
    q.execute(&[items]).unwrap();
    let cmds = commands(&link);
    let copies: Vec<&CopyTextureRegion> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::CopyTextureRegion(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(copies.len(), 2);
    let c0 = copies[0];
    assert_eq!(
        (c0.dst.res, c0.dst.mip, c0.dst.slice, c0.dst.plane),
        (t.res, 0, 2, 0)
    );
    assert_eq!(c0.src.res, up.res);
    assert_eq!(
        (c0.src.offset, c0.src.row_pitch, c0.src.width, c0.src.height),
        (512, 256, 16, 16)
    );
    assert_eq!(c0.src.format, RGBA8);
    assert_eq!(c0.flags, 0);
    let c1 = copies[1];
    assert_eq!((c1.dst.mip, c1.dst.slice), (1, 1));
    assert_eq!((c1.src.mip, c1.src.slice), (1, 2));
    assert_eq!((c1.dst_x, c1.dst_y), (4, 5));
    assert_eq!(c1.flags, CopyTextureRegion::HAS_BOX);
    assert_eq!(
        (
            c1.box_left,
            c1.box_top,
            c1.box_right,
            c1.box_bottom,
            c1.box_back
        ),
        (1, 2, 3, 4, 1)
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::CopyResource(r) if r.dst == t2.res && r.src == t.res))
    );
    let clears: Vec<&ClearUavTexture> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::ClearUavTexture(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(clears.len(), 2);
    assert_eq!(
        (clears[1].rect_left, clears[1].rect_bottom, clears[1].flags),
        (2, 4, ClearUavTexture::HAS_RECT)
    );
    assert_eq!(clears[0].values, [1, 2, 3, 4]);

    // Mistakes surface at Close.
    let mut l = CommandList::new(&dev, 0);
    l.copy_texture_region(
        &CopyLoc::Subresource { res: &t, index: 6 },
        [0; 3],
        &foot,
        None,
    );
    assert_eq!(l.close().err(), Some(Hr::E_INVALIDARG), "no subresource 6");
    let mut l = CommandList::new(&dev, 0);
    l.copy_texture_region(&foot, [0; 3], &foot, None);
    assert_eq!(l.close().err(), Some(Hr::E_INVALIDARG), "buffer to buffer");
    let mut l = CommandList::new(&dev, 0);
    l.copy_resource(&odd, &t);
    assert_eq!(l.close().err(), Some(Hr::E_INVALIDARG), "different sizes");
    let mut l = CommandList::new(&dev, 0);
    l.clear_uav_texture(&t, &texture_uav_of(&t), [0; 4], false, &[[-1, 0, 2, 2]]);
    assert_eq!(l.close().err(), Some(Hr::E_INVALIDARG));
}

fn texture_uav_of(t: &Resource) -> TextureView {
    let mut v = view::default_view(&t.desc, t.res, true);
    v.dim = TexViewDim::TEX2D_ARRAY;
    v.format = RGBA8;
    v
}

#[test]
fn custom_heap_properties_and_the_architecture_answer_agree() {
    use victory_metal_front::caps::{self, Caps};
    let c = Caps::apple();
    let mut data = [0u8; 16];
    caps::check_feature_support(&c, caps::F_ARCHITECTURE, &mut data).unwrap();
    let word = |i: usize| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    let (uma, coherent) = (word(8) != 0, word(12) != 0);
    assert!(uma && coherent);
    for ty in [
        resource::HEAP_DEFAULT,
        resource::HEAP_UPLOAD,
        resource::HEAP_READBACK,
    ] {
        let p = resource::custom_heap_properties(ty, uma).unwrap();
        // On unified memory there is no second pool.
        assert_eq!(p.pool, resource::POOL_L0, "type {ty}");
        let m = p.memory().unwrap();
        assert_eq!(m.cpu_visible(), ty != resource::HEAP_DEFAULT);
    }
}

#[test]
fn default_view_ids_are_not_zero() {
    // The mock numbers resources from 1: a texture is never `ResId(0)`.
    let (_l, dev) = setup();
    let t = dev
        .create_committed(default_props(), &tex2d(4, 4, 1, 1, RGBA8, 0), 0)
        .unwrap();
    assert_ne!(t.res, ResId(0));
}
