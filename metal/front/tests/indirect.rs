//! `CreateCommandSignature` rules and what `ExecuteIndirect` sends the back,
//! against the mock `Link`.

use std::sync::Arc;

use victory_metal_front::device::*;
use victory_metal_front::hresult::Hr;
use victory_metal_front::indirect::{ArgDesc, CommandSignature};
use victory_metal_front::mock::MockLink;
use victory_metal_front::resource::{self, ResourceDesc};
use victory_metal_front::rootsig::{self, Param, ParamKind, RootSignatureDesc, Version};
use victory_metal_front::state;
use victory_metal_proto::indirect::IndirectArgKind;
use victory_metal_proto::{Command, Decoder, RootSigId};

const DRAW: u32 = 0;
const DISPATCH: u32 = 2;
const CONSTANT: u32 = 5;
const CBV: u32 = 6;
const UAV: u32 = 8;

fn setup() -> (Arc<MockLink>, Arc<Device>) {
    let link = Arc::new(MockLink::new());
    let sys = System::open(link.clone()).unwrap();
    (link, Device::new(sys))
}

/// `[CONSTANTS 3, UAV, CBV, TABLE-less]`.
fn root(dev: &Arc<Device>) -> RootSignature {
    let desc = |kind| Param {
        kind,
        visibility: 0,
    };
    let blob = rootsig::serialize(&RootSignatureDesc {
        version: Version::V1_1,
        params: vec![
            desc(ParamKind::Constants {
                reg: 0,
                space: 0,
                num: 3,
            }),
            desc(ParamKind::Descriptor {
                ty: rootsig::PARAM_UAV,
                reg: 0,
                space: 0,
                flags: 0,
            }),
            desc(ParamKind::Descriptor {
                ty: rootsig::PARAM_CBV,
                reg: 0,
                space: 0,
                flags: 0,
            }),
        ],
        samplers: vec![],
        flags: 0,
    })
    .unwrap();
    dev.create_root_signature(&blob).unwrap()
}

fn arg(ty: u32, index: u32, dest_offset: u32, count: u32) -> ArgDesc {
    ArgDesc {
        ty,
        index,
        dest_offset,
        count,
    }
}

#[test]
fn signatures_are_checked_when_they_are_made() {
    let (_link, dev) = setup();
    let rs = root(&dev);
    let ok = |stride, args: &[ArgDesc], root| CommandSignature::new(stride, args, root).is_ok();
    // A dispatch needs no root signature, nor a stride beyond its 12 bytes.
    assert!(ok(12, &[arg(DISPATCH, 0, 0, 0)], None));
    assert!(ok(16, &[arg(DISPATCH, 0, 0, 0)], Some(&rs)));
    // Root arguments need the root signature and parameters of their kind.
    let args = [
        arg(CONSTANT, 0, 1, 2),
        arg(UAV, 1, 0, 0),
        arg(DISPATCH, 0, 0, 0),
    ];
    assert!(!ok(32, &args, None));
    assert!(ok(32, &args, Some(&rs)));
    assert!(!ok(24, &args, Some(&rs)), "stride shorter than the record");
    assert!(!ok(30, &args, Some(&rs)), "not whole words");
    // Constants past the parameter, a CBV on a UAV parameter, a missing parameter.
    assert!(!ok(
        32,
        &[arg(CONSTANT, 0, 2, 2), arg(DISPATCH, 0, 0, 0)],
        Some(&rs)
    ));
    assert!(!ok(
        32,
        &[arg(CBV, 1, 0, 0), arg(DISPATCH, 0, 0, 0)],
        Some(&rs)
    ));
    assert!(!ok(
        32,
        &[arg(UAV, 9, 0, 0), arg(DISPATCH, 0, 0, 0)],
        Some(&rs)
    ));
    assert!(!ok(
        32,
        &[arg(CONSTANT, 1, 0, 1), arg(DISPATCH, 0, 0, 0)],
        Some(&rs)
    ));
    // The action is last and alone, and there is one.
    assert!(!ok(
        32,
        &[arg(DISPATCH, 0, 0, 0), arg(UAV, 1, 0, 0)],
        Some(&rs)
    ));
    assert!(!ok(
        32,
        &[arg(DISPATCH, 0, 0, 0), arg(DISPATCH, 0, 0, 0)],
        None
    ));
    assert!(!ok(32, &[arg(UAV, 1, 0, 0)], Some(&rs)));
    assert!(!ok(32, &[], None));
    assert!(!ok(32, &[arg(77, 0, 0, 0)], None));
    // Draws are valid signatures (the back refuses to run them for now).
    assert!(ok(16, &[arg(DRAW, 0, 0, 0)], None));
    // A root signature given for nothing is not sent.
    let s = CommandSignature::new(12, &[arg(DISPATCH, 0, 0, 0)], Some(&rs)).unwrap();
    assert_eq!(s.root_sig, RootSigId::NULL);
    let s = CommandSignature::new(32, &args, Some(&rs)).unwrap();
    assert_eq!(s.root_sig, rs.id);
    assert_eq!(s.record_bytes, 2 * 4 + 8 + 12);
}

#[test]
fn execute_indirect_sends_the_signature_and_the_buffers() {
    let (link, dev) = setup();
    let rs = root(&dev);
    let args_buf = dev
        .create_committed_buffer(
            resource::HEAP_DEFAULT,
            &ResourceDesc::buffer(4096, resource::FLAG_ALLOW_UNORDERED_ACCESS),
            state::INDIRECT_ARGUMENT,
        )
        .unwrap();
    let count_buf = dev
        .create_committed_buffer(
            resource::HEAP_DEFAULT,
            &ResourceDesc::buffer(256, 0),
            state::INDIRECT_ARGUMENT,
        )
        .unwrap();
    let sig = CommandSignature::new(
        32,
        &[
            arg(CONSTANT, 0, 1, 2),
            arg(UAV, 1, 0, 0),
            arg(DISPATCH, 0, 0, 0),
        ],
        Some(&rs),
    )
    .unwrap();
    let q = dev.create_queue(1, 0).unwrap();
    let mut l = CommandList::new(&dev, 1);
    l.execute_indirect(&sig, 16, &args_buf, 64, Some((&count_buf, 8)));
    l.execute_indirect(&sig, 1, &args_buf, 0, None);
    // Zero records do nothing.
    l.execute_indirect(&sig, 0, &args_buf, 0, None);
    q.execute(&[l.close().unwrap()]).unwrap();
    let block = link.blocks()[0].clone();
    let cmds = Decoder::new(&block).unwrap().collect_all().unwrap();
    let found: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::ExecuteIndirect(e) => Some(*e),
            _ => None,
        })
        .collect();
    assert_eq!(found.len(), 2);
    let e = found[0];
    assert_eq!((e.args, e.args_offset), (args_buf.res, 64));
    assert_eq!((e.count, e.count_offset), (count_buf.res, 8));
    assert_eq!((e.max_count, e.stride, e.root_sig), (16, 32, rs.id));
    let kinds: Vec<_> = e.arguments().map(|a| a.kind).collect();
    assert_eq!(
        kinds,
        [
            IndirectArgKind::CONSTANT,
            IndirectArgKind::UNORDERED_ACCESS_VIEW,
            IndirectArgKind::DISPATCH
        ]
    );
    assert!(found[1].count.is_null());
}

#[test]
fn execute_indirect_with_bad_arguments_fails_the_list_at_close() {
    let (_link, dev) = setup();
    let buf = |size| {
        dev.create_committed_buffer(
            resource::HEAP_DEFAULT,
            &ResourceDesc::buffer(size, 0),
            state::INDIRECT_ARGUMENT,
        )
        .unwrap()
    };
    let small = buf(64);
    let sig = CommandSignature::new(16, &[arg(DISPATCH, 0, 0, 0)], None).unwrap();
    let fails = |f: &dyn Fn(&mut CommandList)| {
        let mut l = CommandList::new(&dev, 1);
        f(&mut l);
        l.close().err()
    };
    // Records past the end of the buffer (no count buffer to stop short).
    assert_eq!(
        fails(&|l| l.execute_indirect(&sig, 5, &small, 0, None)),
        Some(Hr::E_INVALIDARG)
    );
    // An offset that is not whole words.
    assert_eq!(
        fails(&|l| l.execute_indirect(&sig, 1, &small, 2, None)),
        Some(Hr::E_INVALIDARG)
    );
    // A count buffer with no room for the count.
    assert_eq!(
        fails(&|l| l.execute_indirect(&sig, 1, &small, 0, Some((&small, 62)))),
        Some(Hr::E_INVALIDARG)
    );
    // With a count buffer the maximum is only a bound.
    assert_eq!(
        fails(&|l| l.execute_indirect(&sig, 500, &small, 0, Some((&small, 60)))),
        None
    );
}
