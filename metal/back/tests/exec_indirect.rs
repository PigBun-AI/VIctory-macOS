//! `EXECUTE_INDIRECT` of dispatches through the wire, on the GPU: records
//! with only a dispatch, with a count buffer, with root constants and root
//! UAVs that change per record; what must be refused.
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::indirect::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;

/// `{out, tag}`: each group of each dispatch writes `tag + 1` at
/// `out[tag * 8 + group.x]`, so the words say which record ran how many groups.
const KERNELS: &str = "#include <metal_stdlib>\nusing namespace metal;\n\
struct T { ulong out; uint tag; };\n\
kernel void mark(constant T& t [[buffer(2)]], uint3 g [[threadgroup_position_in_grid]]) {\n\
    device uint* o = (device uint*)t.out;\n\
    o[t.tag * 8 + g.x] = t.tag + 1u;\n}\n";

struct Rig {
    fx: Fx,
    queue: QueueId,
    fence: FenceId,
    sig: RootSigId,
    pso: PsoId,
    out: CreateBufferArgs,
    args: CreateBufferArgs,
    count: CreateBufferArgs,
    signal: u64,
}

fn rig() -> Rig {
    let fx = Fx::new();
    let queue = fx.queue();
    let fence = fx.fence(0);
    let sig = fx.root_sig(&RootSigDesc {
        params: vec![descriptor_param(RootParamKind::UAV), constants_param(1)],
        ..RootSigDesc::default()
    });
    let pso = fx
        .executor()
        .create_compute_pipeline_from_msl(sig, KERNELS, "mark", [1, 1, 1])
        .unwrap();
    let out = fx.buffer(BufferKind::SHARED, 4096);
    let args = fx.buffer(BufferKind::SHARED, 4096);
    let count = fx.buffer(BufferKind::SHARED, 256);
    cpu_write(out.cpu_addr, &[0; 1024]);
    Rig {
        fx,
        queue,
        fence,
        sig,
        pso,
        out,
        args,
        count,
        signal: 0,
    }
}

fn arg(kind: IndirectArgKind, index: u32, dest_offset: u32, count: u32) -> IndirectArg {
    IndirectArg {
        kind,
        index,
        dest_offset,
        count,
    }
}

fn encode(args: &[IndirectArg]) -> Vec<u8> {
    let mut v = Vec::new();
    for a in args {
        a.put(&mut v);
    }
    v
}

struct Call<'a> {
    signature: &'a [u8],
    stride: u32,
    max_count: u32,
    count: Option<u64>,
    args_offset: u64,
    root_sig: RootSigId,
}

impl Rig {
    /// A list: the root signature, the pipeline, `out` as root UAV 0, tag
    /// `tag` as root constant, the indirect call, then an ordinary dispatch of
    /// one group (tag 7 unless `after` says) to see what the list's own root
    /// arguments are afterwards. Returns the block's status.
    fn run(&mut self, call: &Call<'_>, tag: u32) -> Status {
        self.signal += 1;
        let tag_bytes = tag.to_le_bytes();
        let c = vec![
            begin(1, self.queue),
            set_sig(self.sig),
            Command::SetPipeline(SetPipeline {
                pso: self.pso,
                _pad: 0,
            }),
            root_va(0, RootParamKind::UAV, self.out.gpu_va),
            Command::SetRootConstants(SetRootConstants {
                bind_point: BindPoint::COMPUTE,
                root_index: 1,
                dest_offset: 0,
                data: &tag_bytes,
            }),
            Command::ExecuteIndirect(ExecuteIndirect {
                args: self.args.res,
                args_offset: call.args_offset,
                count: if call.count.is_some() {
                    self.count.res
                } else {
                    ResId::NULL
                },
                count_offset: call.count.unwrap_or(0),
                max_count: call.max_count,
                stride: call.stride,
                root_sig: call.root_sig,
                signature: call.signature,
            }),
            // After it the list's own arguments are the ones it set: one group, tag `tag`.
            dispatch(1, 1, 1),
            end(1),
            signal_cmd(self.queue, self.fence, self.signal),
        ];
        let s = self.fx.submit(&c);
        if s == Status::OK {
            assert_eq!(self.fx.wait(self.fence, self.signal), Status::OK);
        }
        s
    }

    fn out(&self) -> Vec<u32> {
        cpu_read(self.out.cpu_addr, 1024)
    }

    fn reset(&self) {
        cpu_write(self.out.cpu_addr, &[0; 1024]);
    }
}

/// `out[tag * 8 ..][..8]`.
fn row(out: &[u32], tag: u32) -> &[u32] {
    &out[(tag * 8) as usize..(tag * 8 + 8) as usize]
}

#[test]
fn records_of_dispatches_alone_run_straight_from_the_buffer() {
    let mut r = rig();
    // Three records of {x, y, z}, 16 bytes apart; the tag is the list's: 5.
    cpu_write(r.args.cpu_addr, &[3, 1, 1, 0, 0, 1, 1, 0, 2, 1, 1, 0]);
    let sig = encode(&[arg(IndirectArgKind::DISPATCH, 0, 0, 0)]);
    let call = Call {
        signature: &sig,
        stride: 16,
        max_count: 3,
        count: None,
        args_offset: 0,
        root_sig: RootSigId::NULL,
    };
    assert_eq!(r.run(&call, 5), Status::OK);
    let out = r.out();
    // Records 0 and 2 wrote groups 0..3 and 0..2 of tag 5, then the plain
    // dispatch wrote group 0 again.
    assert_eq!(row(&out, 5), [6, 6, 6, 0, 0, 0, 0, 0]);
    // An offset into the buffer: the second record only.
    r.reset();
    let call = Call {
        args_offset: 16,
        max_count: 1,
        ..call
    };
    assert_eq!(r.run(&call, 2), Status::OK);
    assert_eq!(row(&r.out(), 2), [3, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn a_count_buffer_cuts_the_records_and_is_read_when_the_list_runs() {
    let mut r = rig();
    // Four records of {tag, x, y, z}; root constants come from them.
    cpu_write(
        r.args.cpu_addr,
        &[0, 1, 1, 1, 1, 2, 1, 1, 2, 3, 1, 1, 3, 4, 1, 1],
    );
    let sig = encode(&[
        arg(IndirectArgKind::CONSTANT, 1, 0, 1),
        arg(IndirectArgKind::DISPATCH, 0, 0, 0),
    ]);
    let mut call = Call {
        signature: &sig,
        stride: 16,
        max_count: 4,
        count: Some(8),
        args_offset: 0,
        root_sig: r.sig,
    };
    cpu_write(r.count.cpu_addr, &[99, 99, 2]);
    assert_eq!(r.run(&call, 7), Status::OK);
    let out = r.out();
    assert_eq!(
        row(&out, 0),
        [1, 0, 0, 0, 0, 0, 0, 0],
        "record 0: tag 0, 1 group"
    );
    assert_eq!(
        row(&out, 1),
        [2, 2, 0, 0, 0, 0, 0, 0],
        "record 1: tag 1, 2 groups"
    );
    assert_eq!(row(&out, 2), [0; 8], "past the count");
    assert_eq!(row(&out, 3), [0; 8], "past the count");
    // The plain dispatch after it has the list's own constant (7), not the last record's.
    assert_eq!(row(&out, 7), [8, 0, 0, 0, 0, 0, 0, 0]);

    // A count above the maximum stops at the maximum; a count of 0 runs nothing.
    r.reset();
    cpu_write(r.count.cpu_addr, &[99, 99, 1000]);
    assert_eq!(r.run(&call, 7), Status::OK);
    assert_eq!(row(&r.out(), 3), [4, 4, 4, 4, 0, 0, 0, 0]);
    r.reset();
    cpu_write(r.count.cpu_addr, &[99, 99, 0]);
    assert_eq!(r.run(&call, 7), Status::OK);
    let out = r.out();
    assert!((0..7).all(|t| row(&out, t) == [0; 8]));
    // A count with no root arguments in the records: the kernel only masks.
    let plain = encode(&[arg(IndirectArgKind::DISPATCH, 0, 0, 0)]);
    cpu_write(r.args.cpu_addr, &[2, 1, 1, 0, 3, 1, 1, 0]);
    cpu_write(r.count.cpu_addr, &[99, 99, 1]);
    r.reset();
    call = Call {
        signature: &plain,
        root_sig: RootSigId::NULL,
        max_count: 2,
        ..call
    };
    assert_eq!(r.run(&call, 4), Status::OK);
    assert_eq!(row(&r.out(), 4), [5, 5, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn a_record_changes_root_constants_and_root_descriptors() {
    let mut r = rig();
    let other = r.fx.buffer(BufferKind::SHARED, 4096);
    cpu_write(other.cpu_addr, &[0; 1024]);
    // {uav va (2 words), tag, x, y, z}: 24 bytes, stride 32.
    let mut words = [0u32; 16];
    let va = other.gpu_va;
    words[..6].copy_from_slice(&[va as u32, (va >> 32) as u32, 3, 2, 1, 1]);
    let va = r.out.gpu_va;
    words[8..14].copy_from_slice(&[va as u32, (va >> 32) as u32, 6, 1, 1, 1]);
    cpu_write(r.args.cpu_addr, &words);
    let sig = encode(&[
        arg(IndirectArgKind::UNORDERED_ACCESS_VIEW, 0, 0, 0),
        arg(IndirectArgKind::CONSTANT, 1, 0, 1),
        arg(IndirectArgKind::DISPATCH, 0, 0, 0),
    ]);
    let call = Call {
        signature: &sig,
        stride: 32,
        max_count: 2,
        count: None,
        args_offset: 0,
        root_sig: r.sig,
    };
    assert_eq!(r.run(&call, 1), Status::OK);
    // Record 0 wrote into `other` (tag 3, two groups), record 1 into `out` (tag 6, one).
    let theirs = cpu_read(other.cpu_addr, 1024);
    assert_eq!(row(&theirs, 3), [4, 4, 0, 0, 0, 0, 0, 0]);
    assert!(
        (0..8)
            .filter(|&t| t != 3)
            .all(|t| row(&theirs, t) == [0; 8])
    );
    let out = r.out();
    assert_eq!(row(&out, 6), [7, 0, 0, 0, 0, 0, 0, 0]);
    // The ordinary dispatch afterwards has the list's own bindings: `out`, tag 1.
    assert_eq!(row(&out, 1), [2, 0, 0, 0, 0, 0, 0, 0]);
    // Another pass over the same list shape reuses nothing from the last one.
    r.reset();
    cpu_write(other.cpu_addr, &[0; 1024]);
    assert_eq!(r.run(&call, 4), Status::OK);
    assert_eq!(row(&r.out(), 4), [5, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn many_records_are_expanded_in_chunks() {
    let mut r = rig();
    // 700 records (3 chunks) of {tag, x, y, z} with a count of 650: each of
    // the first 650 writes one word of a big output.
    let n = 700usize;
    let mut words = Vec::new();
    for i in 0..n {
        words.extend_from_slice(&[(i % 100) as u32, 1, 1, 1]);
    }
    let big = r.fx.buffer(BufferKind::SHARED, (n * 16) as u64);
    cpu_write(big.cpu_addr, &words);
    r.args = big;
    cpu_write(r.count.cpu_addr, &[650]);
    let sig = encode(&[
        arg(IndirectArgKind::CONSTANT, 1, 0, 1),
        arg(IndirectArgKind::DISPATCH, 0, 0, 0),
    ]);
    let call = Call {
        signature: &sig,
        stride: 16,
        max_count: n as u32,
        count: Some(0),
        args_offset: 0,
        root_sig: r.sig,
    };
    assert_eq!(r.run(&call, 120), Status::OK);
    let out = r.out();
    // Tags 0..100 ran; each is written by all the records with that tag.
    for tag in 0..100u32 {
        assert_eq!(out[(tag * 8) as usize], tag + 1, "tag {tag}");
    }
}

#[test]
fn what_cannot_run_is_refused_with_a_status() {
    let mut r = rig();
    cpu_write(r.args.cpu_addr, &[1, 1, 1, 0, 1, 1, 1, 0]);
    let dispatch_only = encode(&[arg(IndirectArgKind::DISPATCH, 0, 0, 0)]);
    let base = Call {
        signature: &dispatch_only,
        stride: 16,
        max_count: 2,
        count: None,
        args_offset: 0,
        root_sig: RootSigId::NULL,
    };
    // Arguments past the end of the buffer.
    let past = Call {
        args_offset: 4096 - 8,
        ..Call {
            signature: base.signature,
            ..base
        }
    };
    assert_eq!(r.run(&past, 1), Status::INVALID_ARG);
    // Draws wait for the render encoder: UNSUPPORTED before anything runs.
    let draw = encode(&[arg(IndirectArgKind::DRAW, 0, 0, 0)]);
    let c = Call {
        signature: &draw,
        ..Call {
            signature: base.signature,
            ..base
        }
    };
    assert_eq!(r.run(&c, 1), Status::UNSUPPORTED);
    let mesh = encode(&[arg(IndirectArgKind::DISPATCH_MESH, 0, 0, 0)]);
    let c = Call {
        signature: &mesh,
        ..Call {
            signature: base.signature,
            ..base
        }
    };
    assert_eq!(r.run(&c, 1), Status::UNSUPPORTED);
    let vb = encode(&[
        arg(IndirectArgKind::VERTEX_BUFFER_VIEW, 0, 0, 0),
        arg(IndirectArgKind::DISPATCH, 0, 0, 0),
    ]);
    let c = Call {
        signature: &vb,
        stride: 32,
        ..Call {
            signature: base.signature,
            ..base
        }
    };
    assert_eq!(r.run(&c, 1), Status::UNSUPPORTED);
    // A root parameter that is not what the argument says.
    let wrong = encode(&[
        arg(IndirectArgKind::CONSTANT_BUFFER_VIEW, 0, 0, 0),
        arg(IndirectArgKind::DISPATCH, 0, 0, 0),
    ]);
    let c = Call {
        signature: &wrong,
        stride: 24,
        root_sig: r.sig,
        ..Call {
            signature: base.signature,
            ..base
        }
    };
    assert_eq!(r.run(&c, 1), Status::INVALID_ARG);
    let beyond = encode(&[
        arg(IndirectArgKind::CONSTANT, 1, 1, 1),
        arg(IndirectArgKind::DISPATCH, 0, 0, 0),
    ]);
    let c = Call {
        signature: &beyond,
        stride: 16,
        root_sig: r.sig,
        ..Call {
            signature: base.signature,
            ..base
        }
    };
    assert_eq!(r.run(&c, 1), Status::INVALID_ARG);
    // A signature made for another root signature.
    let other_sig = r.fx.root_sig(&RootSigDesc {
        params: vec![constants_param(2)],
        ..RootSigDesc::default()
    });
    let consts = encode(&[
        arg(IndirectArgKind::CONSTANT, 1, 0, 1),
        arg(IndirectArgKind::DISPATCH, 0, 0, 0),
    ]);
    let c = Call {
        signature: &consts,
        stride: 16,
        root_sig: other_sig,
        ..Call {
            signature: base.signature,
            ..base
        }
    };
    assert_eq!(r.run(&c, 1), Status::INVALID_ARG);
    // Unknown buffers and signatures.
    let c = Call {
        root_sig: RootSigId(999),
        signature: &consts,
        stride: 16,
        ..Call {
            signature: base.signature,
            ..base
        }
    };
    assert_eq!(r.run(&c, 1), Status::NOT_FOUND);
    // Nothing above ran anything.
    assert!(r.out().iter().all(|&w| w == 0));
    // And a good one still works afterwards.
    assert_eq!(r.run(&base, 3), Status::OK);
    assert_eq!(row(&r.out(), 3), [4, 0, 0, 0, 0, 0, 0, 0]);
}
