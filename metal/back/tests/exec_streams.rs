//! The interpreter's command streams: what a block does, what it refuses, and
//! that a refused block leaves nothing behind. Errors are statuses; the
//! executor goes on working after each of them.
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::link::{Link, LinkExt};
use victory_metal_proto::status::Status;
use victory_metal_proto::stream::{Encoder, STREAM_HEADER_SIZE};
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;
use victory_metal_proto::{Caps, PROTO_VERSION};

/// A queue, a fence and a few CPU-visible buffers.
struct World {
    fx: Fx,
    q: QueueId,
    f: FenceId,
    a: CreateBufferArgs,
    b: CreateBufferArgs,
}

fn world() -> World {
    let fx = Fx::new();
    let q = fx.queue();
    let f = fx.fence(0);
    let a = fx.buffer(BufferKind::SHARED, 256);
    let b = fx.buffer(BufferKind::SHARED, 256);
    cpu_write(a.cpu_addr, &[0; 64]);
    cpu_write(b.cpu_addr, &[0; 64]);
    World { fx, q, f, a, b }
}

impl World {
    fn words(&self, buf: &CreateBufferArgs) -> Vec<u32> {
        cpu_read(buf.cpu_addr, 64)
    }

    /// A list that fills `a` with `byte`.
    fn fill_a(&self, list: u32, byte: u8) -> Vec<Command<'static>> {
        vec![
            begin(list, self.q),
            fill(self.a.res, u32::from(byte) * 0x0101_0101, 0, 256),
            end(list),
        ]
    }

    fn a_is(&self, byte: u8) -> bool {
        self.words(&self.a)
            .iter()
            .all(|&w| w == u32::from(byte) * 0x0101_0101)
    }

    /// `a` was not touched.
    fn a_untouched(&self) -> bool {
        self.a_is(0)
    }

    /// Waits for everything the queue has been given.
    fn settle(&self) {
        let v = self.fx.fence_value(self.f) + 1;
        assert_eq!(self.fx.submit(&[signal_cmd(self.q, self.f, v)]), Status::OK);
        assert_eq!(self.fx.wait(self.f, v), Status::OK);
    }
}

/// Header of `cmds` followed by raw bytes counted as `extra` commands.
fn with_tail(cmds: &[Command<'_>], tail: &[u8], extra: u32) -> Vec<u8> {
    let good = block(cmds);
    let count = u32::from_le_bytes(good[28..32].try_into().unwrap());
    block_with(count + extra, &good[STREAM_HEADER_SIZE..])
        .into_iter()
        .chain(tail.iter().copied())
        .collect::<Vec<u8>>()
        .pipe(|mut v| {
            let total = v.len() as u32;
            v[24..28].copy_from_slice(&total.to_le_bytes());
            v
        })
}

trait Pipe: Sized {
    fn pipe<R>(self, f: impl FnOnce(Self) -> R) -> R {
        f(self)
    }
}
impl<T> Pipe for T {}

#[test]
fn a_broken_block_is_refused_whole() {
    let w = world();
    let prefix = w.fill_a(1, 0x11);
    let cases: Vec<(&str, Vec<u8>, Status)> = vec![
        (
            "count says one more",
            with_tail(&prefix, &[], 1),
            Status::BAD_STREAM,
        ),
        (
            "unknown opcode",
            with_tail(&prefix, &raw_cmd(0x7777, 1, 0, 0, &[]), 1),
            Status::BAD_STREAM,
        ),
        (
            "reserved opcode",
            with_tail(&prefix, &raw_cmd(0x0403, 1, 0, 0, &[]), 1),
            Status::UNSUPPORTED,
        ),
        (
            "reserved opcode (ExecuteIndirect)",
            with_tail(&prefix, &raw_cmd(0x0900, 1, 0, 0, &[]), 1),
            Status::UNSUPPORTED,
        ),
        (
            "opcode with a version nobody knows",
            with_tail(&prefix, &raw_cmd(0x0001, 9, 0, 16, &[0; 16]), 1),
            Status::BAD_VERSION,
        ),
        (
            "payload longer than the block",
            with_tail(&prefix, &raw_cmd(0x0002, 1, 0, 800, &[0; 8]), 1),
            Status::BAD_STREAM,
        ),
        (
            "trailing bytes",
            with_tail(&prefix, &[0; 8], 0),
            Status::BAD_STREAM,
        ),
        (
            "cut short",
            {
                let mut v = block(&prefix);
                v.truncate(v.len() - 8);
                v
            },
            Status::BAD_STREAM,
        ),
        (
            "bad magic",
            {
                let mut v = block(&prefix);
                v[0] ^= 0xFF;
                v
            },
            Status::BAD_STREAM,
        ),
        (
            "proto version from the future",
            {
                let mut v = block(&prefix);
                v[8..12].copy_from_slice(&(PROTO_VERSION + 1).to_le_bytes());
                v
            },
            Status::BAD_VERSION,
        ),
        ("shorter than a header", vec![0; 31], Status::BAD_STREAM),
        ("nothing", vec![], Status::BAD_STREAM),
    ];
    for (name, bytes, want) in cases {
        assert_eq!(w.fx.link.submit(&bytes), want, "{name}");
        w.settle();
        assert!(w.a_untouched(), "{name}: the block had an effect");
    }
    // Capability bits the stream was built for.
    let mut e = Encoder::new(Caps(1 << 40));
    for c in &prefix {
        e.push(c).unwrap();
    }
    assert_eq!(
        w.fx.link.submit(&e.finish()),
        Status::BAD_STREAM,
        "unknown capability bit"
    );
    if !cfg!(target_arch = "x86_64") {
        let mut e = Encoder::new(Caps::TRANSLATED);
        for c in &prefix {
            e.push(c).unwrap();
        }
        assert_eq!(
            w.fx.link.submit(&e.finish()),
            Status::UNSUPPORTED,
            "a stream built for Rosetta"
        );
    }
    w.settle();
    assert!(w.a_untouched());
    // The same block, whole, works; so does an empty one.
    assert_eq!(w.fx.submit(&prefix), Status::OK);
    w.settle();
    assert!(w.a_is(0x11));
    assert_eq!(w.fx.submit(&[]), Status::OK);
}

#[test]
fn lists_must_be_well_formed_and_a_bad_block_does_nothing() {
    let w = world();
    let (q, f) = (w.q, w.f);
    let prefix = w.fill_a(1, 0x22);
    let filler = fill(w.b.res, 0x0101_0101, 0, 4);
    let cases: Vec<(&str, Vec<Command<'static>>)> = vec![
        (
            "list inside a list",
            vec![begin(2, q), begin(3, q), end(3), end(2)],
        ),
        ("end without begin", vec![end(2)]),
        ("end of another list", vec![begin(2, q), end(3)]),
        ("never ended", vec![begin(2, q), filler]),
        ("same id twice in one block", vec![begin(1, q), end(1)]),
        ("null id", vec![begin(0, q), end(0)]),
        (
            "signal inside a list",
            vec![begin(2, q), signal_cmd(q, f, 1), end(2)],
        ),
        (
            "wait inside a list",
            vec![begin(2, q), wait_cmd(q, f, 0), end(2)],
        ),
        ("copy outside a list", vec![copy(w.b.res, w.a.res, 16)]),
        ("dispatch outside a list", vec![dispatch(1, 1, 1)]),
        (
            "barrier outside a list",
            vec![Command::Barrier(Barrier::memory(
                stage::BLIT,
                stage::DISPATCH,
            ))],
        ),
        ("root signature outside a list", vec![set_sig(RootSigId(1))]),
    ];
    for (name, tail) in cases {
        let mut cmds = prefix.clone();
        cmds.extend(tail);
        assert_eq!(w.fx.submit(&cmds), Status::BAD_STREAM, "{name}");
        w.settle();
        assert!(w.a_untouched(), "{name}: the lists before it ran");
    }
    assert_eq!(w.words(&w.b)[0], 0);
}

#[test]
fn what_this_back_does_not_do_is_refused_before_anything_runs() {
    let w = world();
    let q = w.q;
    let prefix = w.fill_a(1, 0x33);
    let mut layout = Barrier::memory(stage::BLIT, stage::DISPATCH);
    layout.class = BarrierClass::LAYOUT;
    let gfx = BindPoint::GRAPHICS;
    let sync = Command::SyncRecord(SyncRecord {
        op: SyncOp::NOP,
        status: Status::OK,
        args: &[],
        blob: &[],
    });
    let data = Command::BufferData(BufferData {
        res: w.a.res,
        offset: 0,
        data: &[1, 2, 3],
    });
    let items: Vec<(&str, Command<'static>)> = vec![
        ("SYNC_RECORD", sync),
        ("BUFFER_DATA", data),
        (
            "DISPATCH_INDIRECT",
            Command::DispatchIndirect(DispatchIndirect { args_va: 0x1000 }),
        ),
        (
            "graphics root signature",
            Command::SetRootSignature(SetRootSignature {
                bind_point: gfx,
                root_sig: RootSigId(1),
            }),
        ),
        (
            "graphics root constants",
            Command::SetRootConstants(SetRootConstants {
                bind_point: gfx,
                root_index: 0,
                dest_offset: 0,
                data: &[],
            }),
        ),
        (
            "graphics root descriptor",
            Command::SetRootDescriptor(SetRootDescriptor {
                bind_point: gfx,
                root_index: 0,
                kind: RootParamKind::CBV,
                _pad: 0,
                gpu_va: 0,
            }),
        ),
        (
            "graphics root table",
            Command::SetRootTable(SetRootTable {
                bind_point: gfx,
                root_index: 0,
                heap: HeapId(1),
                index: 0,
            }),
        ),
        ("layout barrier", Command::Barrier(layout)),
    ];
    for (name, item) in items {
        let mut cmds = prefix.clone();
        // In a list of its own, after a list that would run.
        cmds.extend([begin(2, q), item, end(2)]);
        assert_eq!(w.fx.submit(&cmds), Status::UNSUPPORTED, "{name}");
        w.settle();
        assert!(w.a_untouched(), "{name}: the earlier list ran");
    }
    // Outside any list, SYNC_RECORD is just as unsupported.
    let mut cmds = prefix.clone();
    cmds.push(Command::SyncRecord(SyncRecord {
        op: SyncOp::NOP,
        status: Status::OK,
        args: &[],
        blob: &[],
    }));
    assert_eq!(w.fx.submit(&cmds), Status::UNSUPPORTED);
    w.settle();
    assert!(w.a_untouched());
}

#[test]
fn a_semantic_error_drops_the_lists_not_yet_handed_to_the_queue() {
    let w = world();
    let (q, f) = (w.q, w.f);
    let dead = ResId(999);
    // Two lists in one block, the second names a dead buffer: both are lost
    // (one commit never happened).
    let mut cmds = w.fill_a(1, 0x44);
    cmds.extend([begin(2, q), copy(dead, w.a.res, 16), end(2)]);
    assert_eq!(w.fx.submit(&cmds), Status::NOT_FOUND);
    w.settle();
    assert!(w.a_untouched(), "the first list was dropped with the block");
    // The error is in the middle of a list: the open list goes away too and
    // nothing of it leaks into the next block.
    let cmds = [
        begin(1, q),
        fill(w.a.res, 0x5555_5555, 0, 256),
        copy(dead, w.a.res, 16),
        fill(w.b.res, 0x0909_0909, 0, 256),
        end(1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::NOT_FOUND);
    w.settle();
    assert!(w.a_untouched());
    assert_eq!(w.words(&w.b)[0], 0, "commands after the error did not run");
    // A QUEUE_SIGNAL hands what is before it to the queue: that stays.
    let mut cmds = w.fill_a(1, 0x66);
    cmds.push(signal_cmd(q, f, 100));
    cmds.extend([begin(2, q), fill(dead, 0, 0, 4), end(2)]);
    assert_eq!(w.fx.submit(&cmds), Status::NOT_FOUND);
    assert_eq!(w.fx.wait(f, 100), Status::OK);
    assert!(w.a_is(0x66), "lists before a signal were submitted");
    // Bad handles in the queue commands.
    assert_eq!(
        w.fx.submit(&[begin(1, QueueId(99)), end(1)]),
        Status::NOT_FOUND
    );
    assert_eq!(
        w.fx.submit(&[begin(1, QueueId(0)), end(1)]),
        Status::NOT_FOUND
    );
    assert_eq!(
        w.fx.submit(&[signal_cmd(q, FenceId(99), 1)]),
        Status::NOT_FOUND
    );
    assert_eq!(
        w.fx.submit(&[signal_cmd(QueueId(99), f, 1)]),
        Status::NOT_FOUND
    );
    assert_eq!(
        w.fx.submit(&[wait_cmd(q, FenceId(0), 1)]),
        Status::NOT_FOUND
    );
    let mut b = block(&[begin(1, q), end(1)]);
    // `flags` of BEGIN_LIST is reserved.
    let flags_at = STREAM_HEADER_SIZE + 8 + 8;
    b[flags_at] = 1;
    assert_eq!(w.fx.link.submit(&b), Status::INVALID_ARG);
    // And it all still works.
    assert_eq!(w.fx.submit(&w.fill_a(1, 0x77)), Status::OK);
    w.settle();
    assert!(w.a_is(0x77));
}

/// A root signature `[CONSTANTS 2, CBV, TABLE(SRV)]`, a pipeline for it,
/// and heaps of every kind.
struct State {
    w: World,
    sig: RootSigId,
    sig_small: RootSigId,
    pso: PsoId,
    heap: HeapId,
    staging: HeapId,
    sampler: HeapId,
    sampler_sig: RootSigId,
}

fn state() -> State {
    let w = world();
    let sig = w.fx.root_sig(&victory_metal_proto::root::RootSigDesc {
        params: vec![
            constants_param(2),
            descriptor_param(RootParamKind::CBV),
            table_param(0, 1),
        ],
        ranges: vec![range(RangeKind::SRV)],
        ..Default::default()
    });
    let sig_small = w.fx.root_sig(&victory_metal_proto::root::RootSigDesc {
        params: vec![constants_param(1)],
        ..Default::default()
    });
    let sampler_sig = w.fx.root_sig(&victory_metal_proto::root::RootSigDesc {
        params: vec![table_param(0, 1)],
        ranges: vec![range(RangeKind::SAMPLER)],
        ..Default::default()
    });
    let pso = w
        .fx
        .executor()
        .create_compute_pipeline_from_msl(
            sig,
            "#include <metal_stdlib>\nusing namespace metal;\nkernel void k(constant uint* t [[buffer(2)]]) {}",
            "k",
            [1, 1, 1],
        )
        .unwrap();
    let heap = w.fx.heap(HeapKind::CBV_SRV_UAV, 8, true).heap;
    let staging = w.fx.heap(HeapKind::CBV_SRV_UAV, 8, false).heap;
    let sampler = w.fx.heap(HeapKind::SAMPLER, 8, true).heap;
    State {
        w,
        sig,
        sig_small,
        pso,
        heap,
        staging,
        sampler,
        sampler_sig,
    }
}

impl State {
    /// The commands inside one list; the status of the whole block.
    fn in_list(&self, cmds: Vec<Command<'static>>) -> Status {
        let mut all = vec![begin(1, self.w.q)];
        all.extend(cmds);
        all.push(end(1));
        let s = self.w.fx.submit(&all);
        self.w.settle();
        s
    }

    fn pso_cmd(&self, pso: PsoId) -> Command<'static> {
        Command::SetPipeline(SetPipeline { pso, _pad: 0 })
    }

    fn constants(&self, root_index: u32, dest_offset: u32, count: usize) -> Command<'static> {
        const DATA: [u8; 16] = [1; 16];
        Command::SetRootConstants(SetRootConstants {
            bind_point: BindPoint::COMPUTE,
            root_index,
            dest_offset,
            data: &DATA[..4 * count],
        })
    }

    fn table(&self, root_index: u32, heap: HeapId, index: u32) -> Command<'static> {
        Command::SetRootTable(SetRootTable {
            bind_point: BindPoint::COMPUTE,
            root_index,
            heap,
            index,
        })
    }

    fn heaps(&self, resources: HeapId, sampler: HeapId) -> Command<'static> {
        Command::SetDescriptorHeaps(SetDescriptorHeaps {
            cbv_srv_uav: resources,
            sampler,
        })
    }
}

#[test]
fn state_commands_check_what_they_are_given() {
    let s = state();
    let live = s.w.a.gpu_va;
    let (a, b) = (s.w.a.res, s.w.b.res);
    let ok = Status::OK;
    let inv = Status::INVALID_ARG;
    let nf = Status::NOT_FOUND;
    let uns = Status::UNSUPPORTED;
    let with_sig = |mut v: Vec<Command<'static>>| {
        v.insert(0, set_sig(s.sig));
        v
    };
    let cases: Vec<(&str, Vec<Command<'static>>, Status)> = vec![
        // What a dispatch needs.
        ("dispatch with nothing bound", vec![dispatch(1, 1, 1)], inv),
        (
            "dispatch without a root signature",
            vec![s.pso_cmd(s.pso), dispatch(1, 1, 1)],
            inv,
        ),
        (
            "dispatch without a pipeline",
            with_sig(vec![dispatch(1, 1, 1)]),
            inv,
        ),
        (
            "pipeline of another root signature",
            vec![set_sig(s.sig_small), s.pso_cmd(s.pso), dispatch(1, 1, 1)],
            inv,
        ),
        (
            "dispatch with everything (arguments unset: zeros)",
            with_sig(vec![s.pso_cmd(s.pso), dispatch(2, 1, 1)]),
            ok,
        ),
        (
            "dispatch of no groups",
            with_sig(vec![s.pso_cmd(s.pso), dispatch(0, 1, 1)]),
            ok,
        ),
        // Handles.
        ("dead pipeline", vec![s.pso_cmd(PsoId(99))], nf),
        ("null pipeline", vec![s.pso_cmd(PsoId(0))], nf),
        ("dead root signature", vec![set_sig(RootSigId(99))], nf),
        ("null root signature", vec![set_sig(RootSigId(0))], nf),
        (
            "same root signature twice",
            vec![set_sig(s.sig), set_sig(s.sig)],
            ok,
        ),
        // Root constants.
        (
            "constants before a root signature",
            vec![s.constants(0, 0, 1)],
            inv,
        ),
        (
            "constants: root index out of range",
            with_sig(vec![s.constants(9, 0, 1)]),
            inv,
        ),
        (
            "constants on a CBV parameter",
            with_sig(vec![s.constants(1, 0, 1)]),
            inv,
        ),
        (
            "constants on a table parameter",
            with_sig(vec![s.constants(2, 0, 1)]),
            inv,
        ),
        (
            "constants past the parameter",
            with_sig(vec![s.constants(0, 1, 2)]),
            inv,
        ),
        (
            "constants offset far past",
            with_sig(vec![s.constants(0, u32::MAX, 1)]),
            inv,
        ),
        (
            "constants: last one",
            with_sig(vec![s.constants(0, 1, 1)]),
            ok,
        ),
        ("constants: all", with_sig(vec![s.constants(0, 0, 2)]), ok),
        ("constants: none", with_sig(vec![s.constants(0, 2, 0)]), ok),
        // Root descriptors.
        (
            "descriptor before a root signature",
            vec![root_va(1, RootParamKind::CBV, live)],
            inv,
        ),
        (
            "descriptor of the wrong kind",
            with_sig(vec![root_va(1, RootParamKind::UAV, live)]),
            inv,
        ),
        (
            "descriptor on constants",
            with_sig(vec![root_va(0, RootParamKind::CBV, live)]),
            inv,
        ),
        (
            "descriptor on a table",
            with_sig(vec![root_va(2, RootParamKind::SRV, live)]),
            inv,
        ),
        (
            "descriptor: root index out of range",
            with_sig(vec![root_va(7, RootParamKind::CBV, live)]),
            inv,
        ),
        (
            "descriptor: address of nothing",
            with_sig(vec![root_va(1, RootParamKind::CBV, 0x1234)]),
            inv,
        ),
        (
            "descriptor: far from any buffer",
            with_sig(vec![root_va(1, RootParamKind::CBV, live + (1 << 40))]),
            inv,
        ),
        (
            "descriptor: inside a buffer",
            with_sig(vec![root_va(1, RootParamKind::CBV, live + 100)]),
            ok,
        ),
        (
            "descriptor: null address",
            with_sig(vec![root_va(1, RootParamKind::CBV, 0)]),
            ok,
        ),
        // Root tables.
        (
            "table before a root signature",
            vec![s.table(2, s.heap, 0)],
            inv,
        ),
        (
            "table: dead heap",
            with_sig(vec![s.table(2, HeapId(99), 0)]),
            nf,
        ),
        (
            "table: null heap",
            with_sig(vec![s.table(2, HeapId(0), 0)]),
            nf,
        ),
        (
            "table: index past the heap",
            with_sig(vec![s.table(2, s.heap, 8)]),
            inv,
        ),
        (
            "table: last slot",
            with_sig(vec![s.table(2, s.heap, 7)]),
            ok,
        ),
        (
            "table: staging heap is fine (the address is all it needs)",
            with_sig(vec![s.table(2, s.staging, 0)]),
            ok,
        ),
        (
            "table on constants",
            with_sig(vec![s.table(0, s.heap, 0)]),
            inv,
        ),
        (
            "table: sampler heap for resources",
            with_sig(vec![s.table(2, s.sampler, 0)]),
            inv,
        ),
        (
            "table: resource heap for samplers",
            vec![set_sig(s.sampler_sig), s.table(0, s.heap, 0)],
            inv,
        ),
        (
            "table: sampler heap for samplers",
            vec![set_sig(s.sampler_sig), s.table(0, s.sampler, 3)],
            ok,
        ),
        // Descriptor heaps.
        ("heaps: dead", vec![s.heaps(HeapId(99), HeapId(0))], nf),
        (
            "heaps: dead sampler heap",
            vec![s.heaps(s.heap, HeapId(99))],
            nf,
        ),
        (
            "heaps: not shader visible",
            vec![s.heaps(s.staging, HeapId(0))],
            inv,
        ),
        (
            "heaps: sampler heap as resource heap",
            vec![s.heaps(s.sampler, HeapId(0))],
            inv,
        ),
        (
            "heaps: resource heap as sampler heap",
            vec![s.heaps(s.heap, s.heap)],
            inv,
        ),
        ("heaps: none", vec![s.heaps(HeapId(0), HeapId(0))], ok),
        ("heaps: resources", vec![s.heaps(s.heap, HeapId(0))], ok),
        (
            "heaps: samplers only",
            vec![s.heaps(HeapId(0), s.sampler)],
            ok,
        ),
        ("heaps: both", vec![s.heaps(s.heap, s.sampler)], ok),
        // Copies, fills, clears.
        ("copy: dead destination", vec![copy(ResId(99), a, 16)], nf),
        ("copy: dead source", vec![copy(a, ResId(99), 16)], nf),
        ("copy: null buffer", vec![copy(ResId(0), a, 16)], nf),
        ("copy: past the end", vec![copy(a, b, 257)], inv),
        (
            "copy: offset past the end",
            vec![Command::CopyBufferRegion(CopyBufferRegion {
                dst: a,
                src: b,
                dst_offset: u64::MAX,
                src_offset: 0,
                size: 16,
            })],
            inv,
        ),
        (
            "copy: overlap inside one buffer",
            vec![Command::CopyBufferRegion(CopyBufferRegion {
                dst: a,
                src: a,
                dst_offset: 8,
                src_offset: 0,
                size: 16,
            })],
            inv,
        ),
        ("copy: nothing", vec![copy(a, b, 0)], ok),
        ("copy: whole buffer", vec![copy(a, b, 256)], ok),
        ("fill: dead buffer", vec![fill(ResId(99), 0, 0, 4)], nf),
        ("fill: past the end", vec![fill(a, 0, 252, 8)], inv),
        (
            "fill: a pattern a byte cannot make",
            vec![fill(a, 0x0102_0304, 0, 4)],
            uns,
        ),
        ("fill: a byte", vec![fill(a, 0xABAB_ABAB, 0, 4)], ok),
        ("fill: nothing", vec![fill(a, 0, 0, 0)], ok),
        (
            "clear: a format with four channels",
            vec![clear(a, format::R32G32B32A32_UINT, [0; 4], 0, 16)],
            uns,
        ),
        (
            "clear: one channel, a byte value",
            vec![clear(a, format::R32_UINT, [0x0505_0505, 1, 2, 3], 0, 16)],
            ok,
        ),
        (
            "clear: raw",
            vec![clear(a, format::UNKNOWN, [0; 4], 0, 16)],
            ok,
        ),
        (
            "clear: a value a byte cannot make",
            vec![clear(a, format::R32_UINT, [0x0505_0506, 0, 0, 0], 0, 16)],
            uns,
        ),
        (
            "clear: dead buffer",
            vec![clear(ResId(99), format::R32_UINT, [0; 4], 0, 16)],
            nf,
        ),
        (
            "clear: past the end",
            vec![clear(a, format::R32_UINT, [0; 4], 250, 16)],
            inv,
        ),
        // Barriers and labels.
        (
            "barrier",
            vec![Command::Barrier(Barrier::memory(
                stage::BLIT,
                stage::DISPATCH,
            ))],
            ok,
        ),
        (
            "aliasing barrier",
            vec![Command::Barrier(Barrier::aliasing(a, b))],
            ok,
        ),
        (
            "label",
            vec![Command::DebugLabel(b"a frame"), Command::DebugLabel(b"")],
            ok,
        ),
    ];
    for (name, cmds, want) in cases {
        assert_eq!(s.in_list(cmds), want, "{name}");
    }
    // After all that refused work the executor is as good as new.
    assert_eq!(s.w.fx.submit(&s.w.fill_a(1, 0x12)), Status::OK);
    s.w.settle();
    assert!(s.w.a_is(0x12));
}

fn clear(res: ResId, format: u32, values: [u32; 4], offset: u64, size: u64) -> Command<'static> {
    Command::ClearUav(ClearUav {
        res,
        format,
        values,
        offset,
        size,
    })
}

#[test]
fn fills_and_clears_write_what_they_say() {
    let w = world();
    let q = w.q;
    let cmds = [
        begin(1, q),
        fill(w.a.res, 0x0707_0707, 16, 32),
        clear(w.a.res, format::R32_UINT, [0xABAB_ABAB, 0, 0, 0], 64, 32),
        clear(w.a.res, format::UNKNOWN, [0x0202_0202, 0, 0, 0], 128, 16),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::BLIT)),
        copy(w.b.res, w.a.res, 256),
        end(1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    w.settle();
    for buf in [&w.a, &w.b] {
        let words = w.words(buf);
        for (i, &v) in words.iter().enumerate() {
            let want = match i {
                4..=11 => 0x0707_0707,
                16..=23 => 0xABAB_ABAB,
                32..=35 => 0x0202_0202,
                _ => 0,
            };
            assert_eq!(v, want, "word {i}");
        }
    }
}

#[test]
fn many_lists_in_one_block_are_committed_in_pieces_and_stay_in_order() {
    // The queue's pool is 16 command buffers here: 50 lists in one block
    // must not wait for themselves.
    let w = world();
    let q = w.q;
    let mut cmds = Vec::new();
    for i in 0..50u32 {
        cmds.extend([
            begin(i + 1, q),
            fill(w.a.res, (i % 4 + 1) * 0x0101_0101, 4 * u64::from(i % 16), 4),
            end(i + 1),
        ]);
    }
    cmds.push(signal_cmd(q, w.f, 1));
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    assert_eq!(w.fx.wait(w.f, 1), Status::OK);
    // List i wrote word i % 16 with i % 4 + 1; the last writer of each word wins.
    let words = w.words(&w.a);
    for k in 0..16u32 {
        let last = (0..50u32).rfind(|i| i % 16 == k).unwrap();
        assert_eq!(words[k as usize], (last % 4 + 1) * 0x0101_0101, "word {k}");
    }
}

#[test]
fn a_queue_waits_for_a_fence_another_queue_signals() {
    let w = world();
    let q2 = w.fx.queue();
    let (ready, done) = (w.fx.fence(0), w.fx.fence(0));
    // Queue 1 is told to wait first and its work is submitted first.
    let one = [
        wait_cmd(w.q, ready, 1),
        begin(1, w.q),
        fill(w.a.res, 0x0101_0101, 0, 256),
        end(1),
        signal_cmd(w.q, done, 1),
    ];
    assert_eq!(w.fx.submit(&one), Status::OK);
    assert_eq!(w.fx.wait_for(done, 1, 150), Status::TIMEOUT, "it waits");
    assert!(w.a_untouched());
    let two = [
        begin(1, q2),
        fill(w.b.res, 0x0202_0202, 0, 256),
        end(1),
        signal_cmd(q2, ready, 1),
    ];
    assert_eq!(w.fx.submit(&two), Status::OK);
    assert_eq!(w.fx.wait(done, 1), Status::OK);
    assert!(w.a_is(1));
    assert!(w.words(&w.b).iter().all(|&x| x == 0x0202_0202));
}

#[test]
fn every_submit_maintains_the_device() {
    // A destroyed buffer is retired until the queue is done with it, then
    // freed by the next submit, whatever that submit holds.
    let w = world();
    let tmp = w.fx.buffer(BufferKind::SHARED, 256);
    let cmds = [
        begin(1, w.q),
        fill(tmp.res, 0x0101_0101, 0, 256),
        end(1),
        signal_cmd(w.q, w.f, 1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    assert_eq!(w.fx.wait(w.f, 1), Status::OK);
    let mut d = DestroyBufferArgs::new();
    d.res = tmp.res;
    assert_eq!(w.fx.status(d), Status::OK);
    let device = w.fx.executor().device().unwrap();
    assert_eq!(w.fx.submit(&[]), Status::OK);
    assert_eq!(
        device.residency().pending_retired(),
        0,
        "an empty block still maintains"
    );
    // A block that fails maintains too.
    let tmp = w.fx.buffer(BufferKind::SHARED, 256);
    let mut d = DestroyBufferArgs::new();
    d.res = tmp.res;
    assert_eq!(w.fx.status(d), Status::OK);
    assert_eq!(
        w.fx.submit(&[begin(1, w.q), copy(ResId(99), ResId(98), 4), end(1)]),
        Status::NOT_FOUND
    );
    assert_eq!(device.residency().pending_retired(), 0);
}

#[test]
fn a_buffer_destroyed_while_a_list_holds_it_stays_alive_for_the_list() {
    // The block names the buffer and then the buffer is destroyed before the
    // GPU ran it: the work still sees valid memory (nothing crashes, the
    // copy lands).
    let w = world();
    let src = w.fx.buffer(BufferKind::SHARED, 256);
    cpu_write(src.cpu_addr, &[0x1234_5678; 64]);
    let cmds = [
        begin(1, w.q),
        copy(w.a.res, src.res, 256),
        end(1),
        signal_cmd(w.q, w.f, 1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    let mut d = DestroyBufferArgs::new();
    d.res = src.res;
    assert_eq!(w.fx.status(d), Status::OK);
    assert_eq!(w.fx.wait(w.f, 1), Status::OK);
    assert!(w.words(&w.a).iter().all(|&x| x == 0x1234_5678));
}

#[test]
fn several_threads_submit_at_once() {
    let w = world();
    let big = w.fx.buffer(BufferKind::SHARED, 4096);
    cpu_write(big.cpu_addr, &[0; 1024]);
    let queues: Vec<(QueueId, FenceId)> = (0..4).map(|_| (w.fx.queue(), w.fx.fence(0))).collect();
    let threads: Vec<_> = queues
        .iter()
        .enumerate()
        .map(|(t, &(q, f))| {
            let link = w.fx.link.clone();
            let res = big.res;
            std::thread::spawn(move || {
                for i in 0..30u32 {
                    let cmds = [
                        begin(1, q),
                        fill(
                            res,
                            (t as u32 + 1) * 0x0101_0101,
                            1024 * t as u64 + 4 * u64::from(i),
                            4,
                        ),
                        end(1),
                        signal_cmd(q, f, u64::from(i) + 1),
                    ];
                    assert_eq!(link.submit(&block(&cmds)), Status::OK);
                }
                let mut a = FenceWaitArgs::new();
                a.fence = f;
                a.value = 30;
                a.timeout_ms = 20_000;
                link.call_args(&mut a)
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), Status::OK);
    }
    let words = cpu_read(big.cpu_addr, 1024);
    for t in 0..4usize {
        for i in 0..30usize {
            assert_eq!(
                words[256 * t + i],
                (t as u32 + 1) * 0x0101_0101,
                "thread {t} write {i}"
            );
        }
    }
}

/// Time of one `ExecuteCommandLists` block of a 4 KiB copy in the executor,
/// 100 in a row, as the e2e "call only" case makes them. Timing: `#[ignore]`d.
#[test]
#[ignore = "timing"]
fn block_call_time() {
    let fx = Fx::with_options(victory_metal_back::exec::ExecOptions::default());
    let q = fx.queue();
    let a = fx.buffer(BufferKind::SHARED, 4096).res;
    let b = fx.buffer(BufferKind::SHARED, 4096).res;
    let blocks: Vec<Vec<u8>> = (0..100)
        .map(|i| block(&[begin(i + 1, q), copy(a, b, 4096), end(i + 1)]))
        .collect();
    let mut us = Vec::new();
    for round in 0..std::env::var("ROUNDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60)
    {
        let t = std::time::Instant::now();
        for b in &blocks {
            assert_eq!(fx.link.submit(b), Status::OK);
        }
        let dt = t.elapsed().as_secs_f64() * 1e6 / 100.0;
        std::thread::sleep(std::time::Duration::from_millis(30));
        if round >= 10 {
            us.push(dt);
        }
    }
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    println!(
        "PERF block_call_time p50 = {:.2} us / p90 {:.2}",
        us[us.len() / 2],
        us[us.len() * 9 / 10]
    );
}

/// The commit of a block is made by the queue's own thread, after the call
/// has returned. What the calls say still holds: a long chain of copies,
/// each on the other of two queues and ordered only by fence waits and
/// signals, made in one burst without waiting, lands in order; and the fence
/// is visible (a CPU wait on its last value returns) with nothing waited for
/// in between.
#[test]
fn commits_made_off_the_calling_thread_keep_the_order_of_the_calls() {
    let fx = Fx::with_options(victory_metal_back::exec::ExecOptions::default());
    let (q0, q1) = (fx.queue(), fx.queue());
    let f = fx.fence(0);
    const N: usize = 200;
    let bufs: Vec<CreateBufferArgs> = (0..=N)
        .map(|_| fx.buffer(BufferKind::SHARED, 256))
        .collect();
    for b in &bufs {
        cpu_write(b.cpu_addr, &[0; 64]);
    }
    let pattern: Vec<u32> = (0..64).map(|i| 0x1000 + i).collect();
    cpu_write(bufs[0].cpu_addr, &pattern);
    for i in 0..N {
        let q = if i % 2 == 0 { q0 } else { q1 };
        let mut cmds = Vec::new();
        if i > 0 {
            cmds.push(wait_cmd(q, f, i as u64));
        }
        cmds.extend([
            begin(i as u32 + 1, q),
            copy(bufs[i + 1].res, bufs[i].res, 256),
            end(i as u32 + 1),
            signal_cmd(q, f, i as u64 + 1),
        ]);
        assert_eq!(fx.submit(&cmds), Status::OK, "step {i}");
    }
    assert_eq!(fx.wait(f, N as u64), Status::OK);
    assert_eq!(fx.fence_value(f), N as u64);
    assert_eq!(cpu_read(bufs[N].cpu_addr, 64), pattern);
}

fn write_imm(res: ResId, offset: u64, size: u32, value: u64, mode: u32) -> Command<'static> {
    Command::WriteImmediate(WriteImmediate {
        dst: res,
        size,
        offset,
        value,
        mode,
        _pad: 0,
    })
}

#[test]
fn immediate_writes_land_in_command_order_with_both_sizes() {
    let w = world();
    let q = w.q;
    let cmds = [
        begin(1, q),
        fill(w.a.res, 0xFFFF_FFFF, 0, 64),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::BLIT)),
        write_imm(w.a.res, 0, 4, 0x1111_2222, WriteImmediate::DEFAULT),
        write_imm(
            w.a.res,
            8,
            8,
            0x0123_4567_89AB_CDEF,
            WriteImmediate::MARKER_IN,
        ),
        // Overwrites the first write: later commands win.
        write_imm(w.a.res, 0, 4, 0x3333_4444, WriteImmediate::MARKER_OUT),
        end(1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::OK);
    w.settle();
    let words = w.words(&w.a);
    assert_eq!(words[0], 0x3333_4444);
    assert_eq!(words[1], 0xFFFF_FFFF, "four bytes only");
    assert_eq!(words[2], 0x89AB_CDEF);
    assert_eq!(words[3], 0x0123_4567);
    assert_eq!(words[4], 0xFFFF_FFFF);
}

#[test]
fn an_immediate_write_outside_the_buffer_is_refused_and_touches_nothing() {
    let w = world();
    let q = w.q;
    let cmds = [
        begin(1, q),
        write_imm(w.a.res, 252, 8, 1, WriteImmediate::DEFAULT),
        end(1),
    ];
    assert_eq!(w.fx.submit(&cmds), Status::INVALID_ARG);
    w.settle();
    assert!(w.a_untouched());
    // The executor goes on.
    assert_eq!(w.fx.submit(&w.fill_a(2, 0x12)), Status::OK);
    w.settle();
    assert!(w.a_is(0x12));
}
