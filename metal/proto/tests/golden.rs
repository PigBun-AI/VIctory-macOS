//! Golden bytes: the synthetic streams, the dump file, a root signature and a
//! sync block, byte for byte, in `tests/golden/`. They are synthetic (made by
//! `tests/common`), so they may live in the repository.
//!
//! Regenerate after an intended format change (and a `PROTO_VERSION` bump):
//!   UPDATE_GOLDEN=1 cargo test -p victory-metal-proto --test golden

mod common;

use common::*;
use std::path::PathBuf;
use victory_metal_proto::cmd::*;
use victory_metal_proto::file::FileReader;
use victory_metal_proto::ids::*;
use victory_metal_proto::root::RootSigDesc;
use victory_metal_proto::status::Status;
use victory_metal_proto::stream::Decoder;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;
use victory_metal_proto::wire::Wire;
use victory_metal_proto::{Caps, Command};

fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

/// Compares `actual` with the golden file; `UPDATE_GOLDEN=1` rewrites it.
fn golden(name: &str, actual: &[u8]) -> Vec<u8> {
    let p = path(name);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, actual).unwrap();
    }
    let want = std::fs::read(&p).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; run with UPDATE_GOLDEN=1 to create it",
            p.display()
        )
    });
    assert!(
        want == actual,
        "{name} differs from the golden file: the wire format changed. If intended, bump \
         PROTO_VERSION and regenerate (see the header of tests/golden.rs).\n  golden: {} bytes\n  actual: {} bytes",
        want.len(),
        actual.len()
    );
    want
}

#[test]
fn synthetic_compute_stream_is_byte_for_byte_stable() {
    let block = golden("synthetic-compute.vmps", &synthetic_compute());
    // And it decodes to what the scenario says.
    let dec = Decoder::new(&block).unwrap();
    assert_eq!(dec.header().caps, Caps::METAL4 | Caps::UNIFIED_MEMORY);
    let cp = BindPoint::COMPUTE;
    let want: Vec<Command<'_>> = vec![
        Command::QueueWait(QueueWait {
            queue: QueueId(1),
            fence: FenceId(1),
            value: 1,
        }),
        Command::BeginList(BeginList {
            list: ListId(1),
            queue: QueueId(1),
            flags: 0,
            _pad: 0,
        }),
        Command::DebugLabel(b"synthetic compute"),
        Command::CopyBufferRegion(CopyBufferRegion {
            dst: ResId(2),
            src: ResId(1),
            dst_offset: 0,
            src_offset: 0,
            size: 4096,
        }),
        Command::Barrier(Barrier::memory(stage::BLIT, stage::DISPATCH)),
        Command::SetRootSignature(SetRootSignature {
            bind_point: cp,
            root_sig: RootSigId(1),
        }),
        Command::SetPipeline(SetPipeline {
            pso: PsoId(1),
            _pad: 0,
        }),
        Command::SetDescriptorHeaps(SetDescriptorHeaps {
            cbv_srv_uav: HeapId(1),
            sampler: HeapId(0),
        }),
        Command::SetRootConstants(SetRootConstants {
            bind_point: cp,
            root_index: 0,
            dest_offset: 0,
            data: &[0, 4, 0, 0, 0, 0, 0x80, 0x3f],
        }),
        Command::SetRootDescriptor(SetRootDescriptor {
            bind_point: cp,
            root_index: 1,
            kind: RootParamKind::CBV,
            _pad: 0,
            gpu_va: 0x0000_0001_0000_4000,
        }),
        Command::SetRootTable(SetRootTable {
            bind_point: cp,
            root_index: 2,
            heap: HeapId(1),
            index: 0,
        }),
        Command::Dispatch(Dispatch {
            x: 16,
            y: 1,
            z: 1,
            _pad: 0,
        }),
        Command::Barrier(Barrier::memory(stage::DISPATCH, stage::BLIT)),
        Command::CopyBufferRegion(CopyBufferRegion {
            dst: ResId(3),
            src: ResId(2),
            dst_offset: 0,
            src_offset: 0,
            size: 4096,
        }),
        Command::EndList(EndList {
            list: ListId(1),
            _pad: 0,
        }),
        Command::QueueSignal(QueueSignal {
            queue: QueueId(1),
            fence: FenceId(1),
            value: 2,
        }),
    ];
    assert_eq!(dec.collect_all().unwrap(), want);
}

#[test]
fn synthetic_dump_file_is_byte_for_byte_stable() {
    let file = golden("synthetic-dump.vmpf", &synthetic_dump());
    let blocks: Vec<&[u8]> = FileReader::new(&file)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(blocks.len(), 2);
    let setup = Decoder::new(blocks[0]).unwrap().collect_all().unwrap();
    assert_eq!(setup.len(), 3);
    match setup[0] {
        Command::SyncRecord(r) => {
            assert_eq!((r.op, r.status), (SyncOp::CREATE_BUFFER, Status::OK));
            // The recorded block is a CreateBufferArgs with the recorded outputs.
            assert_eq!(r.args, &wire_bytes(&recorded_create_buffer())[..]);
        }
        ref c => panic!("{c:?}"),
    }
    match setup[2] {
        Command::SyncRecord(r) => {
            // The blob is a root signature description.
            assert_eq!(RootSigDesc::decode(r.blob).unwrap(), synthetic_rootsig());
        }
        ref c => panic!("{c:?}"),
    }
}

#[test]
fn synthetic_root_signature_is_byte_for_byte_stable() {
    let bytes = golden(
        "synthetic-rootsig.vmrs",
        &synthetic_rootsig().encode().unwrap(),
    );
    assert_eq!(RootSigDesc::decode(&bytes).unwrap(), synthetic_rootsig());
}

#[test]
fn a_sync_block_is_byte_for_byte_stable() {
    let bytes = golden(
        "create-buffer-args.vmsa",
        &wire_bytes(&recorded_create_buffer()),
    );
    assert_eq!(bytes.len(), CreateBufferArgs::SIZE);
    // Spot-check a few offsets so the golden file means something to a reader:
    // head.version, size, kind, res, gpu_va.
    assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 1);
    assert_eq!(u64::from_le_bytes(bytes[8..16].try_into().unwrap()), 4096);
    assert_eq!(
        u32::from_le_bytes(bytes[16..20].try_into().unwrap()),
        BufferKind::UPLOAD.0
    );
    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 1);
    assert_eq!(
        u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
        0x0000_0001_0000_0000
    );
}
