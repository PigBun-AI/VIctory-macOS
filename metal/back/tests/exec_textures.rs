//! Textures, heaps and samplers through the wire: `CREATE_TEXTURE`,
//! `CREATE_HEAP`, `CREATE_PLACED_BUFFER`, `GET_RESOURCE_ALLOCATION_INFO`,
//! texture and sampler `WRITE_DESCRIPTOR`, `COPY_TEXTURE_REGION`,
//! `COPY_RESOURCE` and `CLEAR_UAV_TEXTURE`. The numbers come back through
//! shared buffers, so a wrong layout shows as wrong words.
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;

struct World {
    fx: Fx,
    q: QueueId,
    f: FenceId,
}

fn world() -> World {
    let fx = Fx::new();
    let q = fx.queue();
    let f = fx.fence(0);
    World { fx, q, f }
}

impl World {
    fn settle(&self) {
        let v = self.fx.fence_value(self.f) + 1;
        assert_eq!(self.fx.submit(&[signal_cmd(self.q, self.f, v)]), Status::OK);
        assert_eq!(self.fx.wait(self.f, v), Status::OK);
    }

    fn texture_args(
        &self,
        dim: TexDim,
        format: u32,
        size: [u32; 3],
        mips: u32,
        usage: u32,
    ) -> CreateTextureArgs {
        let mut a = CreateTextureArgs::new();
        a.dimension = dim;
        a.format = format;
        a.width = size[0];
        a.height = size[1];
        a.depth_or_array = size[2];
        a.mips = mips;
        a.samples = 1;
        a.usage = usage;
        a
    }

    fn texture(
        &self,
        dim: TexDim,
        format: u32,
        size: [u32; 3],
        mips: u32,
        usage: u32,
    ) -> CreateTextureArgs {
        self.fx
            .ok(self.texture_args(dim, format, size, mips, usage))
    }
}

fn sub(res: ResId, mip: u16, slice: u32) -> TextureLocation {
    let mut l = <TextureLocation as victory_metal_proto::wire::Wire>::ZERO;
    l.res = res;
    l.kind = LocKind::SUBRESOURCE;
    l.mip = mip;
    l.slice = slice;
    l
}

fn foot(res: ResId, format: u32, offset: u64, pitch: u32, size: [u32; 3]) -> TextureLocation {
    let mut l = <TextureLocation as victory_metal_proto::wire::Wire>::ZERO;
    l.res = res;
    l.kind = LocKind::FOOTPRINT;
    l.format = format;
    l.offset = offset;
    l.row_pitch = pitch;
    l.width = size[0];
    l.height = size[1];
    l.depth = size[2];
    l
}

fn copy_region(
    dst: TextureLocation,
    src: TextureLocation,
    at: [u32; 3],
    bx: Option<[u32; 6]>,
) -> Command<'static> {
    let mut c = <CopyTextureRegion as victory_metal_proto::wire::Wire>::ZERO;
    c.dst = dst;
    c.src = src;
    c.dst_x = at[0];
    c.dst_y = at[1];
    c.dst_z = at[2];
    if let Some(b) = bx {
        c.flags = CopyTextureRegion::HAS_BOX;
        [
            c.box_left,
            c.box_top,
            c.box_front,
            c.box_right,
            c.box_bottom,
            c.box_back,
        ] = b;
    }
    Command::CopyTextureRegion(c)
}

fn view(kind: ViewKind, res: ResId, format: u32, dim: TexViewDim) -> ViewDesc {
    let mut v = ViewDesc::null();
    v.kind = kind;
    v.resource = res;
    v.format = format;
    v.dimension = dim;
    v.swizzle = swizzle::IDENTITY;
    v
}

#[test]
fn a_texture_round_trips_through_copy_commands_and_has_an_allocation_size() {
    let w = world();
    let t = w.texture(
        TexDim::TEX2D,
        format::R8G8B8A8_UNORM,
        [10, 6, 1],
        2,
        tex_usage::SRV,
    );
    assert!(!t.res.is_null() && t.alloc_size >= 10 * 6 * 4);
    let up = w.fx.buffer(BufferKind::UPLOAD, 4096);
    let rb = w.fx.buffer(BufferKind::READBACK, 4096);
    let words: Vec<u32> = (0..1024)
        .map(|i| 0x0101_0101u32.wrapping_mul(i as u32))
        .collect();
    cpu_write(up.cpu_addr, &words);
    let cmds = [
        begin(1, w.q),
        // Whole mip 0 from rows of 64 bytes (10 texels = 40 used).
        copy_region(
            sub(t.res, 0, 0),
            foot(up.res, format::R8G8B8A8_UNORM, 0, 64, [10, 6, 1]),
            [0; 3],
            None,
        ),
        copy_region(
            sub(t.res, 1, 0),
            foot(up.res, format::R8G8B8A8_UNORM, 1024, 32, [5, 3, 1]),
            [0; 3],
            None,
        ),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::BLIT)),
        copy_region(
            foot(rb.res, format::R8G8B8A8_UNORM, 0, 64, [10, 6, 1]),
            sub(t.res, 0, 0),
            [0; 3],
            None,
        ),
        copy_region(
            foot(rb.res, format::R8G8B8A8_UNORM, 1024, 32, [5, 3, 1]),
            sub(t.res, 1, 0),
            [0; 3],
            None,
        ),
        end(1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    w.settle();
    let got = cpu_read(rb.cpu_addr, 1024);
    for y in 0..6usize {
        for x in 0..10usize {
            assert_eq!(got[y * 16 + x], words[y * 16 + x], "mip0 ({x},{y})");
        }
    }
    for y in 0..3usize {
        for x in 0..5usize {
            assert_eq!(
                got[256 + y * 8 + x],
                words[256 + y * 8 + x],
                "mip1 ({x},{y})"
            );
        }
    }
}

#[test]
fn a_box_selects_the_source_region_and_the_destination_origin_places_it() {
    let w = world();
    let t = w.texture(
        TexDim::TEX2D,
        format::R32_UINT,
        [8, 8, 1],
        1,
        tex_usage::SRV,
    );
    let t2 = w.texture(
        TexDim::TEX2D,
        format::R32_UINT,
        [8, 8, 1],
        1,
        tex_usage::SRV,
    );
    let up = w.fx.buffer(BufferKind::UPLOAD, 4096);
    let rb = w.fx.buffer(BufferKind::READBACK, 4096);
    cpu_write(up.cpu_addr, &(0..64).collect::<Vec<u32>>());
    cpu_write(rb.cpu_addr, &[0; 64]);
    let cmds = [
        begin(1, w.q),
        copy_region(
            sub(t.res, 0, 0),
            foot(up.res, format::R32_UINT, 0, 32, [8, 8, 1]),
            [0; 3],
            None,
        ),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::BLIT)),
        // texels (2..5, 1..3) of t land at (4, 5) of t2.
        copy_region(
            sub(t2.res, 0, 0),
            sub(t.res, 0, 0),
            [4, 5, 0],
            Some([2, 1, 0, 5, 3, 1]),
        ),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::BLIT)),
        copy_region(
            foot(rb.res, format::R32_UINT, 0, 32, [8, 8, 1]),
            sub(t2.res, 0, 0),
            [0; 3],
            None,
        ),
        end(1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    w.settle();
    let got = cpu_read(rb.cpu_addr, 64);
    for y in 0..8u32 {
        for x in 0..8u32 {
            let inside = (4..7).contains(&x) && (5..7).contains(&y);
            let want = if inside {
                (y - 5 + 1) * 8 + (x - 4 + 2)
            } else {
                0
            };
            assert_eq!(got[(y * 8 + x) as usize], want, "({x},{y})");
        }
    }
}

#[test]
fn copy_resource_copies_textures_and_buffers_and_refuses_mixes() {
    let w = world();
    let a = w.texture(
        TexDim::TEX2D,
        format::R32_UINT,
        [4, 4, 1],
        1,
        tex_usage::SRV,
    );
    let b = w.texture(
        TexDim::TEX2D,
        format::R32_UINT,
        [4, 4, 1],
        1,
        tex_usage::SRV,
    );
    let other = w.texture(
        TexDim::TEX2D,
        format::R32_UINT,
        [8, 4, 1],
        1,
        tex_usage::SRV,
    );
    let up = w.fx.buffer(BufferKind::UPLOAD, 256);
    let rb = w.fx.buffer(BufferKind::READBACK, 256);
    cpu_write(up.cpu_addr, &(100..116).collect::<Vec<u32>>());
    let cr = |dst, src| Command::CopyResource(CopyResource { dst, src });
    let cmds = [
        begin(1, w.q),
        copy_region(
            sub(a.res, 0, 0),
            foot(up.res, format::R32_UINT, 0, 16, [4, 4, 1]),
            [0; 3],
            None,
        ),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::BLIT)),
        cr(b.res, a.res),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::BLIT)),
        copy_region(
            foot(rb.res, format::R32_UINT, 0, 16, [4, 4, 1]),
            sub(b.res, 0, 0),
            [0; 3],
            None,
        ),
        end(1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    w.settle();
    assert_eq!(cpu_read(rb.cpu_addr, 16), (100..116).collect::<Vec<u32>>());
    // Refusals leave the executor working.
    for bad in [cr(b.res, other.res), cr(a.res, up.res), cr(up.res, a.res)] {
        let st = w.fx.submit(&[begin(2, w.q), bad, end(2)]);
        assert_eq!(st, Status::INVALID_ARG);
    }
    // A texture onto itself changes nothing and is accepted (the game does it).
    assert_eq!(
        w.fx.submit(&[begin(2, w.q), cr(a.res, a.res), end(2)]),
        Status::OK
    );
    let buf = w.fx.buffer(BufferKind::SHARED, 256);
    assert_eq!(
        w.fx.submit(&[begin(3, w.q), cr(buf.res, up.res), end(3)]),
        Status::OK
    );
    w.settle();
}

#[test]
fn wrong_copy_arguments_are_invalid_arg_not_silent() {
    let w = world();
    let t = w.texture(
        TexDim::TEX2D,
        format::R8G8B8A8_UNORM,
        [8, 8, 1],
        1,
        tex_usage::SRV,
    );
    let up = w.fx.buffer(BufferKind::UPLOAD, 4096);
    let go = |cmd: Command<'static>| w.fx.submit(&[begin(1, w.q), cmd, end(1)]);
    // Pitch below a row; region outside the mip; unknown mip.
    assert_eq!(
        go(copy_region(
            sub(t.res, 0, 0),
            foot(up.res, format::R8G8B8A8_UNORM, 0, 16, [8, 8, 1]),
            [0; 3],
            None
        )),
        Status::INVALID_ARG
    );
    assert_eq!(
        go(copy_region(
            sub(t.res, 0, 0),
            foot(up.res, format::R8G8B8A8_UNORM, 0, 256, [8, 8, 1]),
            [4, 0, 0],
            None
        )),
        Status::INVALID_ARG
    );
    assert_eq!(
        go(copy_region(
            sub(t.res, 3, 0),
            foot(up.res, format::R8G8B8A8_UNORM, 0, 256, [8, 8, 1]),
            [0; 3],
            None
        )),
        Status::INVALID_ARG
    );
    // Buffer to buffer through the texture command, texture handle used as a buffer.
    assert_eq!(
        go(copy_region(
            foot(up.res, format::R32_UINT, 0, 32, [8, 8, 1]),
            foot(up.res, format::R32_UINT, 0, 32, [8, 8, 1]),
            [0; 3],
            None
        )),
        Status::INVALID_ARG
    );
    assert_eq!(
        go(copy_region(
            sub(up.res, 0, 0),
            foot(up.res, format::R32_UINT, 0, 32, [8, 8, 1]),
            [0; 3],
            None
        )),
        Status::INVALID_ARG
    );
    let (st, _) = w.fx.call(DestroyBufferArgs {
        res: t.res,
        ..DestroyBufferArgs::new()
    });
    assert_eq!(
        st,
        Status::INVALID_ARG,
        "a texture is not destroyed as a buffer"
    );
    // Still fine.
    assert_eq!(
        go(copy_region(
            sub(t.res, 0, 0),
            foot(up.res, format::R8G8B8A8_UNORM, 0, 256, [8, 8, 1]),
            [0; 3],
            None
        )),
        Status::OK
    );
    w.settle();
}

#[test]
fn clear_uav_texture_writes_the_requested_region() {
    let w = world();
    let t = w.texture(
        TexDim::TEX2D,
        format::R32_UINT,
        [8, 4, 1],
        1,
        tex_usage::UAV | tex_usage::SRV,
    );
    let rb = w.fx.buffer(BufferKind::READBACK, 256);
    let mut c = <ClearUavTexture as victory_metal_proto::wire::Wire>::ZERO;
    c.res = t.res;
    c.dimension = TexViewDim::TEX2D;
    c.values = [7, 0, 0, 0];
    c.flags = ClearUavTexture::HAS_RECT;
    (c.rect_left, c.rect_top, c.rect_right, c.rect_bottom) = (2, 1, 6, 3);
    let cmds = [
        begin(1, w.q),
        Command::ClearUavTexture(c),
        Command::Barrier(Barrier::memory(stage::DISPATCH, stage::BLIT)),
        copy_region(
            foot(rb.res, format::R32_UINT, 0, 32, [8, 4, 1]),
            sub(t.res, 0, 0),
            [0; 3],
            None,
        ),
        end(1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    w.settle();
    let got = cpu_read(rb.cpu_addr, 32);
    for y in 0..4usize {
        for x in 0..8usize {
            let inside = (2..6).contains(&x) && (1..3).contains(&y);
            // Memory of a new texture is whatever Metal gave: only the cleared region is asserted.
            if inside {
                assert_eq!(got[y * 8 + x], 7, "({x},{y})");
            }
        }
    }
    // A clear the back cannot do exactly says so.
    let mut bad = c;
    bad.float_values = 1;
    assert_eq!(
        w.fx.submit(&[begin(2, w.q), Command::ClearUavTexture(bad), end(2)]),
        Status::UNSUPPORTED
    );
}

#[test]
fn heaps_report_sizes_and_place_buffers_and_textures() {
    let w = world();
    let mut info = GetResourceAllocationInfoArgs::new();
    info.kind = ResKind::TEXTURE;
    info.dimension = TexDim::TEX2D;
    info.format = format::R8G8B8A8_UNORM;
    (
        info.width,
        info.height,
        info.depth_or_array,
        info.mips,
        info.samples,
    ) = (256, 256, 1, 9, 1);
    info.usage = tex_usage::SRV;
    info.heap_kind = MemKind::DEFAULT;
    let info = w.fx.ok(info);
    assert!(
        info.size >= 256 * 256 * 4 && info.alignment.is_power_of_two(),
        "{} {}",
        info.size,
        info.alignment
    );
    let mut binfo = GetResourceAllocationInfoArgs::new();
    binfo.kind = ResKind::BUFFER;
    binfo.buffer_size = 1000;
    binfo.heap_kind = MemKind::DEFAULT;
    let binfo = w.fx.ok(binfo);
    assert!(binfo.size >= 1000);
    // A texture in an upload heap does not exist.
    let mut up_tex = info;
    up_tex.heap_kind = MemKind::UPLOAD;
    assert_eq!(w.fx.status(up_tex), Status::UNSUPPORTED);

    let mut h = CreateHeapArgs::new();
    h.kind = MemKind::DEFAULT;
    h.size = 4 * info.size.max(binfo.size);
    let h = w.fx.ok(h);
    assert!(h.alloc_size >= 4 * info.size);
    let mut t = w.texture_args(
        TexDim::TEX2D,
        format::R8G8B8A8_UNORM,
        [256, 256, 1],
        9,
        tex_usage::SRV,
    );
    t.heap = h.heap;
    t.heap_offset = 0;
    let t = w.fx.ok(t);
    assert!(!t.res.is_null());
    // Misaligned and past-the-end placements are refused.
    let mut bad = w.texture_args(
        TexDim::TEX2D,
        format::R8G8B8A8_UNORM,
        [256, 256, 1],
        9,
        tex_usage::SRV,
    );
    bad.heap = h.heap;
    bad.heap_offset = 1;
    assert_eq!(w.fx.status(bad), Status::INVALID_ARG);
    bad.heap_offset = h.alloc_size;
    assert_eq!(w.fx.status(bad), Status::INVALID_ARG);
    // A placed buffer behind the texture; same heap, other bytes.
    let mut pb = CreatePlacedBufferArgs::new();
    pb.heap = h.heap;
    pb.offset = (2 * info.size).next_multiple_of(binfo.alignment.max(1));
    pb.size = 4096;
    let pb = w.fx.ok(pb);
    assert_ne!(pb.gpu_va, 0);
    assert_eq!(pb.cpu_addr, 0, "a default heap has no CPU pointer");
    // An upload heap places buffers with a CPU pointer.
    let mut uh = CreateHeapArgs::new();
    uh.kind = MemKind::UPLOAD;
    uh.size = 1 << 20;
    let uh = w.fx.ok(uh);
    let mut ub = CreatePlacedBufferArgs::new();
    ub.heap = uh.heap;
    ub.size = 256;
    let ub = w.fx.ok(ub);
    assert_ne!(ub.cpu_addr, 0);
    cpu_write(ub.cpu_addr, &[5; 64]);
    // Destroy in the order the API asks: resources, then the heap.
    assert_eq!(
        w.fx.status(DestroyTextureArgs {
            res: t.res,
            ..DestroyTextureArgs::new()
        }),
        Status::OK
    );
    assert_eq!(
        w.fx.status(DestroyBufferArgs {
            res: pb.res,
            ..DestroyBufferArgs::new()
        }),
        Status::OK
    );
    assert_eq!(
        w.fx.status(DestroyHeapArgs {
            heap: h.heap,
            ..DestroyHeapArgs::new()
        }),
        Status::OK
    );
    assert_eq!(
        w.fx.status(DestroyHeapArgs {
            heap: h.heap,
            ..DestroyHeapArgs::new()
        }),
        Status::NOT_FOUND
    );
}

fn entry(heap: &CreateDescHeapArgs, index: u64) -> [u64; 3] {
    let w = cpu_read_u64s(heap.cpu_addr + 24 * index, 3);
    [w[0], w[1], w[2]]
}

#[test]
fn texture_and_sampler_descriptors_are_written_with_the_view_ids() {
    let w = world();
    let heap = w.fx.heap(HeapKind::CBV_SRV_UAV, 8, true);
    let smp = w.fx.heap(HeapKind::SAMPLER, 4, true);
    let t = w.texture(
        TexDim::TEX2D,
        format::R8G8B8A8_TYPELESS,
        [8, 8, 3],
        4,
        tex_usage::SRV | tex_usage::UAV,
    );
    // Natural view: the texture's own id; the same twice.
    let mut whole = view(
        ViewKind::TEXTURE_SRV,
        t.res,
        format::R8G8B8A8_UNORM,
        TexViewDim::TEX2D_ARRAY,
    );
    whole.mip_count = 0;
    w.fx.write_view(heap.heap, 0, whole);
    let mut srgb = whole;
    srgb.format = format::R8G8B8A8_UNORM_SRGB;
    w.fx.write_view(heap.heap, 1, srgb);
    w.fx.write_view(heap.heap, 2, srgb);
    let (e0, e1, e2) = (entry(&heap, 0), entry(&heap, 1), entry(&heap, 2));
    assert_ne!(e0[1], 0);
    assert_ne!(e0[1], e1[1], "another format is another view");
    assert_eq!(e1, e2, "the same request is the same view");
    // A UAV of one slice of one mip, as a 2D view.
    let mut uav = view(
        ViewKind::TEXTURE_UAV,
        t.res,
        format::R8G8B8A8_UINT,
        TexViewDim::TEX2D,
    );
    (
        uav.mip_first,
        uav.mip_count,
        uav.slice_first,
        uav.slice_count,
    ) = (2, 1, 1, 1);
    w.fx.write_view(heap.heap, 3, uav);
    assert_ne!(entry(&heap, 3)[1], 0);
    // SRV min LOD goes into the metadata word.
    let mut lod = whole;
    lod.min_lod = 2.0;
    w.fx.write_view(heap.heap, 4, lod);
    assert_eq!(entry(&heap, 4)[2] & 0xffff_ffff, 2.0f32.to_bits() as u64);
    // Refusals.
    let mut wrong_family = whole;
    wrong_family.format = format::R16G16_FLOAT;
    assert_eq!(
        w.fx.write_view_status(heap.heap, 5, wrong_family),
        Status::INVALID_ARG
    );
    let mut past = whole;
    past.slice_first = 3;
    assert_eq!(
        w.fx.write_view_status(heap.heap, 5, past),
        Status::INVALID_ARG
    );
    let mut buf_as_tex = whole;
    let b = w.fx.buffer(BufferKind::DEFAULT, 256);
    buf_as_tex.resource = b.res;
    assert_eq!(
        w.fx.write_view_status(heap.heap, 5, buf_as_tex),
        Status::INVALID_ARG
    );
    let no_uav = w.texture(
        TexDim::TEX2D,
        format::R32_UINT,
        [4, 4, 1],
        1,
        tex_usage::SRV,
    );
    let uav_of_srv = view(
        ViewKind::TEXTURE_UAV,
        no_uav.res,
        format::R32_UINT,
        TexViewDim::TEX2D,
    );
    assert_eq!(
        w.fx.write_view_status(heap.heap, 5, uav_of_srv),
        Status::INVALID_ARG
    );
    let ghost = view(
        ViewKind::TEXTURE_SRV,
        ResId(999),
        format::R32_UINT,
        TexViewDim::TEX2D,
    );
    assert_eq!(
        w.fx.write_view_status(heap.heap, 5, ghost),
        Status::NOT_FOUND
    );
    let tex_in_sampler_heap = whole;
    assert_eq!(
        w.fx.write_view_status(smp.heap, 0, tex_in_sampler_heap),
        Status::INVALID_ARG
    );

    // Samplers: equal descriptions share a state; bias and states differ.
    let mut s = ViewDesc::null();
    s.kind = ViewKind::SAMPLER;
    let write_sampler = |index: u32, d: SamplerDesc| {
        let mut a = WriteDescriptorArgs::new();
        a.heap = smp.heap;
        a.index = index;
        a.view = s;
        a.sampler = d;
        w.fx.status(a)
    };
    let base = SamplerDesc {
        min_filter: Filter::LINEAR,
        mag_filter: Filter::LINEAR,
        mip_filter: Filter::LINEAR,
        address_u: AddressMode::CLAMP,
        address_v: AddressMode::CLAMP,
        address_w: AddressMode::CLAMP,
        max_lod: 1000.0,
        ..<SamplerDesc as victory_metal_proto::wire::Wire>::ZERO
    };
    assert_eq!(write_sampler(0, base), Status::OK);
    assert_eq!(write_sampler(1, base), Status::OK);
    let mut nearest = base;
    nearest.min_filter = Filter::NEAREST;
    assert_eq!(write_sampler(2, nearest), Status::OK);
    let (s0, s1, s2) = (entry(&smp, 0), entry(&smp, 1), entry(&smp, 2));
    assert_eq!(s0, s1);
    assert_ne!(s0[0], s2[0]);
    assert_ne!(s0[0], 0);
    // Min/max reduction filters have no Metal equivalent: refused, not approximated.
    let mut reduce = base;
    reduce.reduction = victory_metal_proto::types::Reduction::MIN;
    assert_eq!(write_sampler(3, reduce), Status::UNSUPPORTED);
    reduce.reduction = victory_metal_proto::types::Reduction::MAX;
    assert_eq!(write_sampler(3, reduce), Status::UNSUPPORTED);
}
