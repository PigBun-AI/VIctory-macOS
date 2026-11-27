//! Every opcode and every sync op survives encode -> decode unchanged.

mod common;

use common::*;
use std::collections::BTreeSet;
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::registry::{COMMANDS, SYNC_OPS};
use victory_metal_proto::root::*;
use victory_metal_proto::status::{Error, Status};
use victory_metal_proto::stream::{Decoder, Encoder, STREAM_HEADER_SIZE};
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;
use victory_metal_proto::wire::{Layout, Reader, Wire};
use victory_metal_proto::{Caps, Command, Link, LinkExt};

#[test]
fn every_command_round_trips_alone() {
    for cmd in one_of_each() {
        let block = encode(std::slice::from_ref(&cmd));
        let back = Decoder::new(&block).unwrap().collect_all().unwrap();
        assert_eq!(back, vec![cmd], "{:?}", cmd.opcode());
    }
}

#[test]
fn all_commands_round_trip_in_one_block() {
    let cmds = one_of_each();
    let block = encode(&cmds);
    let dec = Decoder::new(&block).unwrap();
    assert_eq!(dec.header().count as usize, cmds.len());
    assert_eq!(dec.collect_all().unwrap(), cmds);
}

#[test]
fn the_samples_cover_every_known_opcode() {
    let seen: BTreeSet<u16> = one_of_each().iter().map(|c| c.opcode().0).collect();
    let known: BTreeSet<u16> = Opcode::KNOWN.iter().map(|k| k.1).collect();
    assert_eq!(seen, known, "add a sample for the new opcode");
}

#[test]
fn registry_lists_every_opcode_once_with_a_version() {
    let listed: Vec<u16> = COMMANDS.iter().map(|c| c.opcode.0).collect();
    let known: Vec<u16> = Opcode::KNOWN.iter().map(|k| k.1).collect();
    assert_eq!(listed, known, "registry order and opcode list must match");
    for c in COMMANDS {
        assert!(c.version >= 1, "{:?}", c.opcode);
        assert_eq!(c.version, opcode_version(c.opcode));
        assert_eq!(c.size % 8, 0, "{}", c.name);
    }
    // Opcodes that are not known have version 0.
    assert_eq!(opcode_version(Opcode(0x0000)), 0);
    assert_eq!(opcode_version(Opcode(0x0400)), 0);
}

#[test]
fn registry_lists_every_sync_op_once() {
    let listed: Vec<u32> = SYNC_OPS.iter().map(|s| s.op.0).collect();
    let known: Vec<u32> = SyncOp::KNOWN.iter().map(|k| k.1).collect();
    assert_eq!(listed, known);
    for s in SYNC_OPS {
        assert_eq!(s.size % 8, 0, "{}", s.name);
        assert_eq!(s.version, 1);
        assert_eq!(s.fields[0].name, "head");
    }
}

#[test]
fn offsets_in_a_stream_are_multiples_of_eight() {
    let block = encode(&one_of_each());
    assert_eq!(block.len() % 8, 0);
    let mut pos = STREAM_HEADER_SIZE;
    while pos < block.len() {
        assert_eq!(pos % 8, 0);
        let len = u32::from_le_bytes(block[pos + 4..pos + 8].try_into().unwrap()) as usize;
        assert_eq!(len % 8, 0);
        pos += 8 + len;
    }
    assert_eq!(pos, block.len());
}

#[test]
fn root_constants_helper_matches_the_command() {
    let mut e = Encoder::new(Caps::NONE);
    e.set_root_constants(BindPoint::COMPUTE, 3, 1, &[10, 20, 0xFFFF_FFFF])
        .unwrap();
    let block = e.finish();
    let cmds = Decoder::new(&block).unwrap().collect_all().unwrap();
    match cmds[0] {
        Command::SetRootConstants(c) => {
            assert_eq!(c.count(), 3);
            assert_eq!(c.values().collect::<Vec<_>>(), vec![10, 20, 0xFFFF_FFFF]);
            assert_eq!((c.root_index, c.dest_offset), (3, 1));
        }
        ref other => panic!("{other:?}"),
    }
    // The limit is enforced and leaves the block as it was.
    let mut e = Encoder::new(Caps::NONE);
    let too_many = [0u32; MAX_ROOT_CONSTANTS + 1];
    assert_eq!(
        e.set_root_constants(BindPoint::COMPUTE, 0, 0, &too_many),
        Err(Error::TooLarge)
    );
    assert_eq!(e.count(), 0);
    assert_eq!(e.len(), STREAM_HEADER_SIZE);
}

#[test]
fn an_empty_block_round_trips() {
    let block = Encoder::new(Caps::METAL4).finish();
    assert_eq!(block.len(), STREAM_HEADER_SIZE);
    let dec = Decoder::new(&block).unwrap();
    assert_eq!(dec.header().caps, Caps::METAL4);
    assert_eq!(dec.collect_all().unwrap(), vec![]);
}

#[test]
fn the_encoder_refuses_what_check_refuses_and_stays_usable() {
    let mut e = Encoder::new(Caps::NONE);
    let bad = Command::Barrier(Barrier {
        before: 0x0100,
        after: stage::ALL,
        ..Barrier::default()
    });
    assert!(e.push(&bad).is_err());
    assert_eq!(e.count(), 0);
    e.push(&Command::Dispatch(Dispatch {
        x: 1,
        y: 1,
        z: 1,
        _pad: 0,
    }))
    .unwrap();
    let block = e.finish();
    assert_eq!(
        Decoder::new(&block).unwrap().collect_all().unwrap().len(),
        1
    );
}

#[test]
fn root_signature_round_trips() {
    let rs = synthetic_rootsig();
    let bytes = rs.encode().unwrap();
    assert_eq!(bytes.len(), rs.encoded_len());
    assert_eq!(bytes.len() % 8, 0);
    assert_eq!(RootSigDesc::decode(&bytes).unwrap(), rs);
    let empty = RootSigDesc::default();
    assert_eq!(
        RootSigDesc::decode(&empty.encode().unwrap()).unwrap(),
        empty
    );
}

#[test]
fn root_signature_rejects_bad_input() {
    let good = synthetic_rootsig().encode().unwrap();
    // Truncated at every length, and with trailing bytes.
    for n in 0..good.len() {
        assert!(RootSigDesc::decode(&good[..n]).is_err(), "prefix {n}");
    }
    let mut longer = good.clone();
    longer.extend_from_slice(&[0; 8]);
    assert_eq!(RootSigDesc::decode(&longer), Err(Error::TrailingBytes));
    // A count that claims more than the bytes hold.
    let mut huge = good.clone();
    huge[4..8].copy_from_slice(&60u32.to_le_bytes());
    assert_eq!(RootSigDesc::decode(&huge), Err(Error::Truncated));
    // Counts over the limits.
    let mut over = good.clone();
    over[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(RootSigDesc::decode(&over).is_err());
    // Unknown flag bits.
    let mut flags = good.clone();
    flags[0..4].copy_from_slice(&0x80u32.to_le_bytes());
    assert!(RootSigDesc::decode(&flags).is_err());
    // A table that points past the range array.
    let mut rs = synthetic_rootsig();
    rs.params[2].range_count = 3;
    assert!(RootSigDesc::decode(&rs.encode().unwrap()).is_err());
    rs.params[2].range_start = u32::MAX;
    rs.params[2].range_count = 2;
    assert!(RootSigDesc::decode(&rs.encode().unwrap()).is_err());
    // An unknown parameter kind.
    let mut rs = synthetic_rootsig();
    rs.params[0].kind = RootParamKind(99);
    assert!(RootSigDesc::decode(&rs.encode().unwrap()).is_err());
    // Too many parameters to encode.
    let rs = RootSigDesc {
        params: vec![RootParam::default(); MAX_ROOT_PARAMS + 1],
        ..RootSigDesc::default()
    };
    assert_eq!(rs.encode(), Err(Error::TooLarge));
}

// ---- sync blocks ---------------------------------------------------------

/// Bytes -> block -> bytes is the identity, the block's memory is its wire
/// form, and `new()` sets the version.
fn check_block<A: SyncArgs>() {
    let pattern: Vec<u8> = (0..A::SIZE)
        .map(|i| (i as u8).wrapping_mul(7).wrapping_add(1))
        .collect();
    let block = A::get(&mut Reader::new(&pattern)).unwrap();
    assert_eq!(wire_bytes(&block), pattern, "{}", A::NAME);
    // Memory layout equals the wire form (the blocks cross `Link::call` as raw memory).
    assert_eq!(std::mem::size_of::<A>(), A::SIZE);
    assert!(std::mem::align_of::<A>() <= 8);
    let raw = unsafe { std::slice::from_raw_parts((&block as *const A).cast::<u8>(), A::SIZE) };
    assert_eq!(
        raw,
        &pattern[..],
        "{}: memory differs from the wire form",
        A::NAME
    );
    assert_eq!(A::new_zeroed_version().head().version, A::VERSION);
    // Truncated bytes fail.
    assert!(A::get(&mut Reader::new(&pattern[..A::SIZE - 1])).is_err());
    // The same block inside a recorded call survives a stream.
    let rec = Command::SyncRecord(SyncRecord {
        op: A::OP,
        status: Status::OK,
        args: &pattern,
        blob: &[],
    });
    let stream = encode(&[rec]);
    let got = Decoder::new(&stream).unwrap().collect_all().unwrap();
    assert_eq!(got, vec![rec]);
    // The registry knows it with the same numbers.
    let info = SYNC_OPS
        .iter()
        .find(|s| s.op == A::OP)
        .expect("in the registry");
    assert_eq!(
        (info.name, info.size, info.version),
        (A::NAME, A::SIZE, A::VERSION)
    );
}

trait NewZeroedVersion: SyncArgs {
    fn new_zeroed_version() -> Self {
        let mut a = Self::ZERO;
        a.head_mut().version = Self::VERSION;
        a
    }
}
impl<A: SyncArgs> NewZeroedVersion for A {}

#[test]
fn every_sync_block_round_trips() {
    check_block::<NopArgs>();
    check_block::<HelloArgs>();
    check_block::<QueryCapsArgs>();
    check_block::<ShutdownArgs>();
    check_block::<CreateQueueArgs>();
    check_block::<DestroyQueueArgs>();
    check_block::<CreateBufferArgs>();
    check_block::<DestroyBufferArgs>();
    check_block::<WrapMemoryArgs>();
    check_block::<CreateDescHeapArgs>();
    check_block::<DestroyDescHeapArgs>();
    check_block::<WriteDescriptorArgs>();
    check_block::<CopyDescriptorsArgs>();
    check_block::<CreateRootSignatureArgs>();
    check_block::<DestroyRootSignatureArgs>();
    check_block::<CreateComputePipelineArgs>();
    check_block::<DestroyPipelineArgs>();
    check_block::<CreateFenceArgs>();
    check_block::<DestroyFenceArgs>();
    check_block::<FenceValueArgs>();
    check_block::<FenceWaitArgs>();
    check_block::<FenceSignalArgs>();
    check_block::<DeviceStatusArgs>();
    check_block::<GetSurfaceLayerArgs>();
    check_block::<CreateGraphicsPipelineArgs>();
    // Nothing was forgotten: one block type per registry row.
    assert_eq!(SYNC_OPS.len(), SyncOp::KNOWN.len());
}

#[test]
fn field_values_survive_where_set() {
    let mut a = WriteDescriptorArgs::new();
    a.heap = HeapId(2);
    a.index = 9;
    a.view = ViewDesc::typed(ViewKind::BUFFER_UAV, ResId(5), format::R32_FLOAT, 4, 100);
    a.sampler.border_color = [0.0, 0.5, 1.0, -1.0];
    let bytes = wire_bytes(&a);
    let b = WriteDescriptorArgs::get(&mut Reader::new(&bytes)).unwrap();
    assert_eq!(a, b);
    assert!(b.view.is_valid());
    assert_eq!(b.view.format, 41);
    assert_eq!(ViewDesc::cbv(0x1000, 256).kind, ViewKind::CBV);
    assert_eq!(ViewDesc::null().kind, ViewKind::NONE);
    assert_eq!(
        ViewDesc::raw(ViewKind::BUFFER_SRV, ResId(1), 0, 8).form,
        BufferForm::RAW
    );
    assert_eq!(
        ViewDesc::structured(ViewKind::BUFFER_SRV, ResId(1), 16, 0, 8).structure_stride,
        16
    );
}

// ---- Link ----------------------------------------------------------------

/// A back stand-in: answers CREATE_BUFFER and FENCE_VALUE through raw pointers.
struct Mock;

impl Link for Mock {
    fn submit(&self, block: &[u8]) -> Status {
        match Decoder::new(block).and_then(|d| d.collect_all()) {
            Ok(_) => Status::OK,
            Err(e) => e.status(),
        }
    }

    #[allow(unsafe_code)]
    fn call(&self, op: SyncOp, args: *mut u8, len: usize) -> Status {
        match op {
            SyncOp::CREATE_BUFFER => {
                let a = match unsafe { args_from_raw::<CreateBufferArgs>(args, len) } {
                    Ok(a) => a,
                    Err(s) => return s,
                };
                if a.kind == BufferKind::WRAPPED.0 {
                    return Status::INVALID_ARG;
                }
                a.res = ResId(1);
                a.gpu_va = 0x1_0000_0000;
                a.cpu_addr = if a.kind == BufferKind::DEFAULT.0 {
                    0
                } else {
                    0x2_0000_0000
                };
                a.alloc_size = a.size.next_multiple_of(16384);
                Status::OK
            }
            SyncOp::FENCE_VALUE => {
                let a = match unsafe { args_from_raw::<FenceValueArgs>(args, len) } {
                    Ok(a) => a,
                    Err(s) => return s,
                };
                a.value = u64::from(a.fence.0) * 100;
                Status::OK
            }
            _ => Status::UNKNOWN_OP,
        }
    }
}

#[test]
fn a_typed_call_goes_through_the_raw_link() {
    let link: &dyn Link = &Mock;
    let mut a = CreateBufferArgs::new();
    a.size = 100;
    a.kind = BufferKind::UPLOAD.0;
    assert_eq!(link.call_args(&mut a), Status::OK);
    assert_eq!(
        (a.res, a.gpu_va, a.cpu_addr, a.alloc_size),
        (ResId(1), 0x1_0000_0000, 0x2_0000_0000, 16384)
    );
    let mut f = FenceValueArgs::new();
    f.fence = FenceId(3);
    assert_eq!(link.call_args(&mut f), Status::OK);
    assert_eq!(f.value, 300);
    // call_args fills in the version itself.
    let mut z = FenceValueArgs::ZERO;
    assert_eq!(link.call_args(&mut z), Status::OK);
    // Unknown to the mock.
    assert_eq!(link.call_args(&mut NopArgs::new()), Status::UNKNOWN_OP);
}

#[test]
#[allow(unsafe_code)]
fn raw_blocks_are_checked() {
    let link = Mock;
    // A fresh block per case, so no pointer outlives a use of the block.
    let try_call = |version: u16, len: usize| {
        let mut a = CreateBufferArgs::new();
        a.head.version = version;
        link.call(
            SyncOp::CREATE_BUFFER,
            (&mut a as *mut CreateBufferArgs).cast::<u8>(),
            len,
        )
    };
    let size = CreateBufferArgs::SIZE;
    assert_eq!(try_call(1, size), Status::OK);
    assert_eq!(try_call(1, size - 8), Status::INVALID_ARG);
    assert_eq!(try_call(1, size + 8), Status::INVALID_ARG);
    assert_eq!(try_call(1, 0), Status::INVALID_ARG);
    assert_eq!(try_call(2, size), Status::BAD_VERSION);
    assert_eq!(try_call(0, size), Status::BAD_VERSION);
    assert_eq!(
        link.call(SyncOp::CREATE_BUFFER, std::ptr::null_mut(), size),
        Status::INVALID_ARG
    );
    // A misaligned pointer is refused, not read.
    let mut buf = [0u64; 16];
    let q = unsafe { (buf.as_mut_ptr() as *mut u8).add(4) };
    assert_eq!(
        link.call(SyncOp::CREATE_BUFFER, q, size),
        Status::INVALID_ARG
    );
    // A wrapped buffer cannot be created.
    let mut w = CreateBufferArgs::new();
    w.kind = BufferKind::WRAPPED.0;
    assert_eq!(link.call_args(&mut w), Status::INVALID_ARG);
}

#[test]
fn submit_accepts_a_good_block_and_names_a_bad_one() {
    let link = Mock;
    assert_eq!(link.submit(&synthetic_compute()), Status::OK);
    assert_eq!(link.submit(&[1, 2, 3]), Status::BAD_STREAM);
    let mut block = synthetic_compute();
    block[8..12].copy_from_slice(&99u32.to_le_bytes());
    assert_eq!(link.submit(&block), Status::BAD_VERSION);
}

#[test]
fn layouts_name_their_fields() {
    // The generated docs rely on this: padding is spelled `_pad…`, offsets add up.
    for s in SYNC_OPS {
        let mut end = 0;
        for f in s.fields {
            assert_eq!(f.offset, end, "{}.{}", s.name, f.name);
            end += f.size;
        }
        assert_eq!(end, s.size, "{}", s.name);
    }
    assert_eq!(<Barrier as Layout>::NAME, "Barrier");
}

#[test]
fn enums_know_their_names() {
    assert_eq!(BufferKind::UPLOAD.name(), Some("UPLOAD"));
    assert!(!BufferKind(77).is_known());
    assert_eq!(format!("{:?}", BufferKind(77)), "BufferKind(0x4d)");
    assert_eq!(format!("{:?}", Opcode::DISPATCH), "Opcode::DISPATCH");
    assert_eq!(stage::ALL, 0x00FF);
    assert_eq!(
        stage::VERTEX
            | stage::FRAGMENT
            | stage::TILE
            | stage::OBJECT
            | stage::MESH
            | stage::DISPATCH
            | stage::BLIT
            | stage::RESOURCE_STATE,
        stage::ALL
    );
    assert!(Caps::METAL4.contains(Caps::NONE));
    assert!(!Caps::METAL4.contains(Caps::MESH_SHADERS));
}
