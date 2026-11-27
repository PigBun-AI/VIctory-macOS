//! Replaying the proto crate's synthetic golden files through the interpreter:
//! `synthetic-dump.vmpf` (a recorded sync call, buffer data, a recorded root
//! signature, then the compute stream), `synthetic-compute.vmps`,
//! `synthetic-rootsig.vmrs`, `create-buffer-args.vmsa`.
//!
//! The replayer is test code, not part of the back: a dump holds the *front's*
//! handles and addresses, the back hands out its own, so something has to
//! re-execute the recorded sync calls, remap handles and GPU addresses in the
//! stream that follows, and apply the recorded CPU writes (`BUFFER_DATA`).
//! The back's interpreter refuses `SYNC_RECORD` and `BUFFER_DATA` itself.
#![cfg(target_vendor = "apple")]

mod exec_support;

use std::collections::HashMap;
use std::path::PathBuf;

use exec_support::*;
use victory_metal_proto::Caps;
use victory_metal_proto::cmd::*;
use victory_metal_proto::file::FileReader;
use victory_metal_proto::ids::*;
use victory_metal_proto::link::{Link, LinkExt};
use victory_metal_proto::status::Status;
use victory_metal_proto::stream::{Decoder, Encoder};
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;
use victory_metal_proto::wire::{Reader, Wire};

#[path = "../../proto/tests/common/hexfile.rs"]
mod hexfile;

/// A golden of the proto crate (hex text, see `hexfile.rs`).
fn golden(name: &str) -> Vec<u8> {
    hexfile::read(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../proto/tests/golden")
            .join(format!("{name}.txt")),
    )
}

#[derive(Default)]
struct Maps {
    res: HashMap<u32, u32>,
    queue: HashMap<u32, u32>,
    fence: HashMap<u32, u32>,
    sig: HashMap<u32, u32>,
}

/// Re-executes recorded sync calls and runs the blocks of a dump.
struct Replayer<'a> {
    fx: &'a Fx,
    maps: Maps,
    /// (recorded base, length, new base) of every buffer the dump created.
    spans: Vec<(u64, u64, u64)>,
    /// New CPU address of every buffer the replay made.
    cpu: HashMap<u32, u64>,
    /// Addresses the dump holds that no buffer owns (synthetic dumps cheat).
    va_overrides: HashMap<u64, u64>,
}

fn id(map: &HashMap<u32, u32>, v: u32) -> u32 {
    // Handles the dump did not create (the harness made them with the numbers
    // the dump expects) stay as they are.
    map.get(&v).copied().unwrap_or(v)
}

impl<'a> Replayer<'a> {
    fn new(fx: &'a Fx) -> Self {
        Replayer {
            fx,
            maps: Maps::default(),
            spans: Vec::new(),
            cpu: HashMap::new(),
            va_overrides: HashMap::new(),
        }
    }

    fn va(&self, v: u64) -> u64 {
        if v == 0 {
            return 0;
        }
        if let Some(&n) = self.va_overrides.get(&v) {
            return n;
        }
        for &(base, len, new) in &self.spans {
            if (base..base + len).contains(&v) {
                return new + (v - base);
            }
        }
        panic!("the dump uses address {v:#x}, which no replayed buffer owns");
    }

    /// Sync records and CPU writes happen where they stand in the block; the
    /// commands between them go to the back as blocks of their own.
    fn replay_block(&mut self, block: &[u8]) -> Status {
        let mut pending = Encoder::new(Caps::NONE);
        let mut status = Status::OK;
        for c in Decoder::new(block).unwrap() {
            match c.unwrap() {
                Command::SyncRecord(r) => {
                    status = self.flush(&mut pending);
                    if status != Status::OK {
                        return status;
                    }
                    self.sync(&r);
                }
                Command::BufferData(d) => {
                    status = self.flush(&mut pending);
                    if status != Status::OK {
                        return status;
                    }
                    let res = id(&self.maps.res, d.res.0);
                    let cpu = *self
                        .cpu
                        .get(&res)
                        .expect("BUFFER_DATA into a buffer with no CPU address");
                    // SAFETY: the buffer is live, CPU visible, and the range was
                    // taken from the dump; the test sizes its buffers for it.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            d.data.as_ptr(),
                            (cpu + d.offset) as *mut u8,
                            d.data.len(),
                        )
                    };
                }
                other => pending.push(&self.remap(other)).unwrap(),
            }
        }
        if status == Status::OK {
            status = self.flush(&mut pending);
        }
        status
    }

    fn flush(&mut self, pending: &mut Encoder) -> Status {
        if pending.is_empty() {
            return Status::OK;
        }
        let block = std::mem::replace(pending, Encoder::new(Caps::NONE)).finish();
        self.fx.link.submit(&block)
    }

    fn remap<'c>(&self, c: Command<'c>) -> Command<'c> {
        let q = |v: QueueId| QueueId(id(&self.maps.queue, v.0));
        let f = |v: FenceId| FenceId(id(&self.maps.fence, v.0));
        let r = |v: ResId| ResId(if v.0 == 0 { 0 } else { id(&self.maps.res, v.0) });
        match c {
            Command::BeginList(mut b) => {
                b.queue = q(b.queue);
                Command::BeginList(b)
            }
            Command::QueueSignal(mut s) => {
                s.queue = q(s.queue);
                s.fence = f(s.fence);
                Command::QueueSignal(s)
            }
            Command::QueueWait(mut s) => {
                s.queue = q(s.queue);
                s.fence = f(s.fence);
                Command::QueueWait(s)
            }
            Command::SetRootSignature(mut s) => {
                s.root_sig = RootSigId(id(&self.maps.sig, s.root_sig.0));
                Command::SetRootSignature(s)
            }
            Command::SetRootDescriptor(mut d) => {
                d.gpu_va = self.va(d.gpu_va);
                Command::SetRootDescriptor(d)
            }
            Command::DispatchIndirect(mut d) => {
                d.args_va = self.va(d.args_va);
                Command::DispatchIndirect(d)
            }
            Command::CopyBufferRegion(mut c) => {
                c.dst = r(c.dst);
                c.src = r(c.src);
                Command::CopyBufferRegion(c)
            }
            Command::FillBuffer(mut c) => {
                c.dst = r(c.dst);
                Command::FillBuffer(c)
            }
            Command::ClearUav(mut c) => {
                c.res = r(c.res);
                Command::ClearUav(c)
            }
            Command::Barrier(mut b) => {
                b.res_before = r(b.res_before);
                b.res_after = r(b.res_after);
                Command::Barrier(b)
            }
            // Pipelines, heaps and the rest: the dump's numbers are the
            // harness's numbers.
            other => other,
        }
    }

    /// Re-executes one recorded sync call and remembers what it made.
    fn sync(&mut self, r: &SyncRecord<'_>) {
        fn parse<A: SyncArgs>(bytes: &[u8]) -> A {
            // The recorded block, outputs and all; a recorder that did not
            // fill the head (the synthetic one) is repaired by `call_args`.
            A::get(&mut Reader::new(bytes)).expect("recorded argument block")
        }
        let status;
        match r.op {
            SyncOp::NOP => status = self.fx.status(parse::<NopArgs>(r.args)),
            SyncOp::CREATE_BUFFER => {
                let mut a = parse::<CreateBufferArgs>(r.args);
                let (rec_res, rec_va) = (a.res.0, a.gpu_va);
                status = self.fx.link.call_args(&mut a);
                if status == Status::OK {
                    self.maps.res.insert(rec_res, a.res.0);
                    self.spans.push((rec_va, a.size, a.gpu_va));
                    if a.cpu_addr != 0 {
                        self.cpu.insert(a.res.0, a.cpu_addr);
                    }
                }
            }
            SyncOp::CREATE_QUEUE => {
                let mut a = parse::<CreateQueueArgs>(r.args);
                let rec = a.queue.0;
                status = self.fx.link.call_args(&mut a);
                self.maps.queue.insert(rec, a.queue.0);
            }
            SyncOp::CREATE_FENCE => {
                let mut a = parse::<CreateFenceArgs>(r.args);
                let rec = a.fence.0;
                status = self.fx.link.call_args(&mut a);
                self.maps.fence.insert(rec, a.fence.0);
            }
            SyncOp::CREATE_ROOT_SIGNATURE => {
                let mut a = parse::<CreateRootSignatureArgs>(r.args);
                let rec = a.root_sig.0;
                a.data_addr = r.blob.as_ptr() as u64;
                a.data_len = r.blob.len() as u64;
                status = self.fx.link.call_args(&mut a);
                // A record with no output handle (the synthetic one has zeros)
                // relies on the back counting from 1 like the dump's author did.
                if rec != 0 {
                    self.maps.sig.insert(rec, a.root_sig.0);
                }
            }
            op => panic!("this replayer does not know {op:?}"),
        }
        assert_eq!(
            status, r.status,
            "{:?} answered differently from the recording",
            r.op
        );
    }
}

const KERNEL: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct Entry { ulong va; ulong tex; ulong md; };\n\
struct Tlab { uint n; float scale; ulong cbv; ulong table; };\n\
kernel void scale_add(constant Tlab& t [[buffer(2)]], uint i [[thread_position_in_grid]]) {\n\
    device const Entry* e = (device const Entry*)t.table;\n\
    device const uint* src = (device const uint*)e[0].va;\n\
    device uint* out = (device uint*)e[1].va;\n\
    uint bias = *(device const uint*)t.cbv;\n\
    if (i < t.n) out[i] = (uint)((float)src[i] * t.scale) + bias;\n}\n";

const BIAS: u32 = 11;

/// Objects the compute stream expects to exist (the dump does not make them):
/// queue 1, fence 1 already at 1, buffers 2 (GPU) and 3 (readback), heap 1,
/// pipeline 1; plus a buffer for the root CBV address the dump invented.
struct Harness {
    readback: CreateBufferArgs,
    cbv: CreateBufferArgs,
}

fn harness(fx: &Fx) -> Harness {
    assert_eq!(fx.queue(), QueueId(1));
    assert_eq!(fx.fence(1), FenceId(1));
    let out = fx.buffer(BufferKind::DEFAULT, 4096);
    let readback = fx.buffer(BufferKind::READBACK, 4096);
    assert_eq!((out.res, readback.res), (ResId(2), ResId(3)));
    let cbv = fx.buffer(BufferKind::SHARED, 256);
    cpu_write(cbv.cpu_addr, &[BIAS]);
    let heap = fx.heap(HeapKind::CBV_SRV_UAV, 8, true);
    assert_eq!(heap.heap, HeapId(1));
    fx.write_view(
        heap.heap,
        0,
        ViewDesc::raw(ViewKind::BUFFER_SRV, ResId(1), 0, 1024),
    );
    fx.write_view(
        heap.heap,
        1,
        ViewDesc::raw(ViewKind::BUFFER_UAV, out.res, 0, 1024),
    );
    let pso = fx
        .executor()
        .create_compute_pipeline_from_msl(RootSigId(1), KERNEL, "scale_add", [64, 1, 1])
        .unwrap();
    assert_eq!(pso, PsoId(1));
    Harness { readback, cbv }
}

/// Runs the dump's two blocks with the harness in between; returns what the
/// copy-back left in buffer 3.
fn run_dump(fx: &Fx) -> Vec<u32> {
    let file = golden("synthetic-dump.vmpf");
    let blocks: Vec<&[u8]> = FileReader::new(&file)
        .unwrap()
        .map(|b| b.unwrap())
        .collect();
    assert_eq!(blocks.len(), 2);
    let mut rp = Replayer::new(fx);
    // Block 0: a recorded CREATE_BUFFER (upload, 4096 bytes), eight bytes of
    // buffer data, a recorded CREATE_ROOT_SIGNATURE.
    assert_eq!(rp.replay_block(blocks[0]), Status::OK);
    assert_eq!(rp.maps.res.get(&1), Some(&1), "recorded buffer 1 is ours 1");
    let src = *rp.cpu.get(&1).expect("an upload buffer has a CPU address");
    assert_eq!(
        cpu_read(src, 2),
        [0x0403_0201, 0x0807_0605],
        "BUFFER_DATA landed in the buffer"
    );
    let h = harness(fx);
    let words: Vec<u32> = (0..1024u32).map(|i| i * 3).collect();
    cpu_write(src, &words);
    // The compute stream's root CBV address (0x1_0000_4000) belongs to no
    // buffer of the dump: point it at the buffer the harness made.
    rp.va_overrides.insert(0x0000_0001_0000_4000, h.cbv.gpu_va);
    assert_eq!(rp.replay_block(blocks[1]), Status::OK);
    // The stream signals fence 1 with 2 after the copy back.
    assert_eq!(fx.wait(FenceId(1), 2), Status::OK);
    let got = cpu_read(h.readback.cpu_addr, 1024);
    // The root constants are [1024, 1.0f]: out[i] = src[i] * 1.0 + bias.
    let want: Vec<u32> = words.iter().map(|&v| v + BIAS).collect();
    assert_eq!(got, want);
    got
}

#[test]
fn the_synthetic_dump_replays_and_computes() {
    let fx = Fx::new();
    run_dump(&fx);
}

#[test]
fn a_replay_is_repeatable_across_sessions() {
    let fx = Fx::new();
    let first = run_dump(&fx);
    assert_eq!(fx.status(ShutdownArgs::new()), Status::OK);
    let mut h = HelloArgs::new();
    h.client_proto_version = victory_metal_proto::PROTO_VERSION;
    assert_eq!(fx.status(h), Status::OK);
    assert_eq!(run_dump(&fx), first);
}

#[test]
fn the_compute_stream_alone_names_objects_that_do_not_exist() {
    // Replayed into a fresh session: queue 1 does not exist, so the very
    // first command (QUEUE_WAIT) is refused and nothing runs.
    let fx = Fx::new();
    let block = golden("synthetic-compute.vmps");
    assert_eq!(fx.link.submit(&block), Status::NOT_FOUND);
    // With a queue and fence but no buffers/heaps/pipeline the list fails
    // at its first copy.
    assert_eq!(fx.queue(), QueueId(1));
    assert_eq!(fx.fence(1), FenceId(1));
    assert_eq!(fx.link.submit(&block), Status::NOT_FOUND);
    assert_eq!(
        fx.fence_value(FenceId(1)),
        1,
        "the final signal never happened"
    );
}

#[test]
fn golden_sync_blocks_replay_as_single_calls() {
    let fx = Fx::new();
    // create-buffer-args.vmsa: a recorded CREATE_BUFFER block.
    let bytes = golden("create-buffer-args.vmsa");
    let mut a = CreateBufferArgs::get(&mut Reader::new(&bytes)).unwrap();
    assert_eq!((a.size, a.kind), (4096, BufferKind::UPLOAD.0));
    assert_eq!(fx.link.call_args(&mut a), Status::OK);
    assert_ne!(a.res.0, 0);
    assert_ne!(
        a.gpu_va, 0x0000_0001_0000_0000,
        "the back chooses addresses"
    );
    assert!(a.alloc_size >= 4096);
    // synthetic-rootsig.vmrs: the root signature bytes.
    let rs = golden("synthetic-rootsig.vmrs");
    let (s, sig) = fx.root_sig_status(&rs);
    assert_eq!(s, Status::OK);
    let l = fx.executor().tlab_layout(sig).unwrap();
    // MSC reflection: constants (2) at 0, the CBV pointer at 8, the table
    // pointer at 16, then the pointer to the static-sampler table at 24.
    assert_eq!(
        (0..3)
            .map(|i| l.slot(i).unwrap().offset)
            .collect::<Vec<_>>(),
        [0, 8, 16]
    );
    assert_eq!(l.size(), 32);
    assert_eq!(l.static_table_offset(), Some(24));
    // The recorded block of the dump's second call is not a valid root
    // signature when its blob is cut: refused, not a panic.
    for cut in [0, 1, 15, 16, rs.len() - 1] {
        assert_eq!(
            fx.root_sig_status(&rs[..cut]).0,
            Status::INVALID_ARG,
            "{cut} bytes"
        );
    }
}

#[test]
fn every_prefix_of_the_golden_files_is_a_status() {
    // The decoder's own fuzz test covers the framing; here the interpreter
    // is fed every cut of the real files and must answer, never fall over.
    let fx = Fx::new();
    let compute = golden("synthetic-compute.vmps");
    for len in (0..compute.len()).step_by(8) {
        let s = fx.link.submit(&compute[..len]);
        assert_ne!(s, Status::OK, "{len} bytes");
        assert!(s.0 < 0);
    }
    // A flipped byte anywhere in the stream: any status, no panic.
    for at in (0..compute.len()).step_by(3) {
        let mut b = compute.clone();
        b[at] ^= 0xA5;
        let _ = fx.link.submit(&b);
    }
    // The executor still works.
    assert_eq!(fx.status(NopArgs::new()), Status::OK);
}
