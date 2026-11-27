//! The core against a mock `Link`: what the front sends for what the
//! application does. Golden bytes: tests/golden/compute-block.vmps
//! (`UPDATE_GOLDEN=1 cargo test -p victory-metal-front --test device`).

#[path = "../../proto/tests/common/hexfile.rs"]
mod hexfile;

use std::sync::Arc;

use victory_metal_front::descriptor::{self, HeapDesc};
use victory_metal_front::device::*;
use victory_metal_front::hresult::Hr;
use victory_metal_front::mock::{DESCRIPTOR_STRIDE, Descriptor, MockLink};
use victory_metal_front::resource::{self, ResourceDesc};
use victory_metal_front::rootsig::{self, Param, ParamKind, RootSignatureDesc, Version};
use victory_metal_front::state;
use victory_metal_proto::cmd::*;
use victory_metal_proto::sync::SyncOp;
use victory_metal_proto::types::{BindPoint, RootParamKind, ViewKind, stage};
use victory_metal_proto::{
    Command, Decoder, FenceId, HeapId, ListId, PsoId, QueueId, ResId, RootSigId, Status,
};

fn setup() -> (Arc<MockLink>, Arc<Device>) {
    let link = Arc::new(MockLink::new());
    let sys = System::open(link.clone()).unwrap();
    (link, Device::new(sys))
}

fn root_sig_blob() -> Vec<u8> {
    rootsig::serialize(&RootSignatureDesc {
        version: Version::V1_1,
        params: vec![
            Param {
                kind: ParamKind::Constants {
                    reg: 0,
                    space: 0,
                    num: 3,
                },
                visibility: 0,
            },
            Param {
                kind: ParamKind::Descriptor {
                    ty: rootsig::PARAM_UAV,
                    reg: 0,
                    space: 0,
                    flags: 0,
                },
                visibility: 0,
            },
        ],
        samplers: vec![],
        flags: 0,
    })
    .unwrap()
}

fn buf(dev: &Arc<Device>, heap: u32, size: u64, uav: bool, state: u32) -> Buffer {
    let flags = if uav {
        resource::FLAG_ALLOW_UNORDERED_ACCESS
    } else {
        0
    };
    dev.create_committed_buffer(heap, &ResourceDesc::buffer(size, flags), state)
        .unwrap()
}

#[test]
fn connecting_reports_the_back_and_its_limits() {
    let (link, dev) = setup();
    assert_eq!(link.calls_of(SyncOp::HELLO), 1);
    assert_eq!(dev.sys.adapter.name, victory_metal_front::mock::DEVICE_NAME);
    assert_eq!(
        dev.sys.adapter.dedicated_video_memory,
        victory_metal_front::mock::WORKING_SET
    );
    assert_eq!(
        dev.descriptor_increment(descriptor::TYPE_CBV_SRV_UAV),
        DESCRIPTOR_STRIDE
    );
    assert_eq!(dev.sys.caps.wave_lane_min, 32);
}

#[test]
fn a_back_that_refuses_hello_is_no_adapter() {
    let link = Arc::new(MockLink::new());
    link.fail_next(SyncOp::HELLO, Status::UNSUPPORTED);
    assert!(System::open(link).is_err());
}

#[test]
fn buffers_live_and_die_with_their_objects() {
    let (link, dev) = setup();
    let up = buf(
        &dev,
        resource::HEAP_UPLOAD,
        256,
        false,
        resource::STATE_GENERIC_READ,
    );
    let def = buf(&dev, resource::HEAP_DEFAULT, 4096, true, 0);
    assert_ne!(up.gpu_va, def.gpu_va);
    assert_ne!(up.cpu_addr, 0);
    assert_eq!(up.map(0), Ok(up.cpu_addr));
    assert_eq!(
        def.map(0),
        Err(Hr::E_INVALIDARG),
        "a DEFAULT buffer has no CPU address"
    );
    assert_eq!(link.live_buffers(), 2);
    drop(up);
    assert_eq!(link.live_buffers(), 1);
    drop(def);
    assert_eq!(link.live_buffers(), 0);
    // Rules from D3D12 are checked before the back is asked.
    let before = link.calls_of(SyncOp::CREATE_BUFFER);
    assert!(
        dev.create_committed_buffer(resource::HEAP_UPLOAD, &ResourceDesc::buffer(16, 0), 0)
            .is_err()
    );
    assert_eq!(link.calls_of(SyncOp::CREATE_BUFFER), before);
}

#[test]
fn back_errors_become_hresults() {
    let (link, dev) = setup();
    link.fail_next(SyncOp::CREATE_BUFFER, Status::OUT_OF_MEMORY);
    let r = dev.create_committed_buffer(resource::HEAP_DEFAULT, &ResourceDesc::buffer(16, 0), 0);
    assert_eq!(r.err(), Some(Hr::E_OUTOFMEMORY));
}

#[test]
fn root_signature_and_pipeline_reach_the_back_as_proto() {
    let (link, dev) = setup();
    let rs = dev.create_root_signature(&root_sig_blob()).unwrap();
    let p = link.root_signature(rs.id).unwrap();
    assert_eq!(p.params.len(), 2);
    assert_eq!(p.params[0].kind, RootParamKind::CONSTANTS);
    assert_eq!(p.params[0].num_32bit_values, 3);
    assert_eq!(p.params[1].kind, RootParamKind::UAV);
    assert!(dev.create_root_signature(b"garbage").is_err());
    let pso = dev.create_compute_pipeline(&rs, b"DXBC.......").unwrap();
    assert_eq!(link.live_pipelines(), 1);
    assert!(dev.create_compute_pipeline(&rs, b"nope").is_err());
    drop(pso);
    drop(rs);
    assert_eq!(link.live_pipelines(), 0);
    assert_eq!(link.live_root_signatures(), 0);
}

#[test]
fn descriptors_go_through_handles_to_heap_and_slot() {
    let (link, dev) = setup();
    let heap = dev
        .create_descriptor_heap(&HeapDesc {
            ty: descriptor::TYPE_CBV_SRV_UAV,
            count: 8,
            flags: descriptor::FLAG_SHADER_VISIBLE,
            node_mask: 0,
        })
        .unwrap();
    let cpu = |i: u64| {
        heap.cpu_start() + i * dev.descriptor_increment(descriptor::TYPE_CBV_SRV_UAV) as u64
    };
    let b = buf(&dev, resource::HEAP_DEFAULT, 4096, true, 0);
    dev.create_buffer_view(
        cpu(3),
        &BufferView {
            uav: true,
            resource: Some(b.res),
            first_element: 0,
            num_elements: 1024,
            structure_stride: 0,
            format: 0,
            raw: true,
            counter: None,
        },
    )
    .unwrap();
    let Some(Descriptor::View(v)) = link.descriptor(heap.back, 3) else {
        panic!("slot 3 not written")
    };
    assert_eq!(
        (v.kind, v.resource, v.num_elements),
        (ViewKind::BUFFER_UAV, b.res, 1024)
    );
    dev.create_cbv(cpu(0), b.gpu_va, 256).unwrap();
    dev.copy_descriptors(cpu(5), cpu(3), 1).unwrap();
    assert_eq!(link.descriptor(heap.back, 5), link.descriptor(heap.back, 3));
    // A handle that is not on a descriptor is refused before the back sees it.
    assert_eq!(
        dev.create_cbv(cpu(1) + 1, b.gpu_va, 256),
        Err(Hr::E_INVALIDARG)
    );
    assert_eq!(dev.create_cbv(cpu(8), b.gpu_va, 256), Err(Hr::E_INVALIDARG));
    // Typed and structured views.
    let typed = BufferView {
        uav: false,
        resource: Some(b.res),
        first_element: 2,
        num_elements: 8,
        structure_stride: 0,
        format: 42,
        raw: false,
        counter: None,
    };
    dev.create_buffer_view(cpu(1), &typed).unwrap();
    let st = BufferView {
        structure_stride: 16,
        format: 0,
        ..typed
    };
    dev.create_buffer_view(cpu(2), &st).unwrap();
    assert!(
        dev.create_buffer_view(cpu(4), &BufferView { format: 0, ..typed })
            .is_err()
    );
    drop(heap);
    assert_eq!(link.live_heaps(), 0);
}

/// The golden compute stream: copy in, UAV barrier, dispatch, copy out.
fn record_compute(
    dev: &Arc<Device>,
    link: &MockLink,
) -> (Vec<u8>, [ResId; 3], RootSigId, PsoId, QueueId) {
    let up = buf(
        dev,
        resource::HEAP_UPLOAD,
        4096,
        false,
        resource::STATE_GENERIC_READ,
    );
    let def = buf(dev, resource::HEAP_DEFAULT, 4096, true, 0);
    let rb = buf(
        dev,
        resource::HEAP_READBACK,
        4096,
        false,
        resource::STATE_COPY_DEST,
    );
    let rs = dev.create_root_signature(&root_sig_blob()).unwrap();
    let pso = dev.create_compute_pipeline(&rs, b"DXBC.......").unwrap();
    let q = dev.create_queue(1, 0).unwrap();
    let mut l = CommandList::new(dev, 1);
    use ResourceBarrier::*;
    l.resource_barrier(&[Transition {
        res: def.res,
        subresource: 0xffff_ffff,
        before: 0,
        after: state::COPY_DEST,
        flags: 0,
    }]);
    l.copy_buffer_region(&def, 0, &up, 0, 4096);
    l.resource_barrier(&[Transition {
        res: def.res,
        subresource: 0xffff_ffff,
        before: state::COPY_DEST,
        after: state::UNORDERED_ACCESS,
        flags: 0,
    }]);
    l.set_compute_root_signature(&rs);
    l.set_pipeline(&pso);
    l.set_compute_root_constants(0, 0, &[1, 2, 3]);
    l.set_compute_root_descriptor(1, RootParamKind::UAV, def.gpu_va);
    l.dispatch(4, 2, 1);
    l.resource_barrier(&[Uav { res: Some(def.res) }]);
    l.resource_barrier(&[Transition {
        res: def.res,
        subresource: 0xffff_ffff,
        before: state::UNORDERED_ACCESS,
        after: state::COPY_SOURCE,
        flags: 0,
    }]);
    l.copy_buffer_region(&rb, 0, &def, 0, 4096);
    let items = l.close().unwrap();
    q.execute(&[items]).unwrap();
    let blocks = link.blocks();
    assert_eq!(blocks.len(), 1);
    let ids = [up.res, def.res, rb.res];
    let (rsid, psoid, qid) = (rs.id, pso.id, q.id);
    // Leak the objects' Drop on purpose: ids stay valid for the assertions.
    std::mem::forget((up, def, rb, rs, pso, q));
    (blocks[0].clone(), ids, rsid, psoid, qid)
}

#[test]
fn a_compute_list_encodes_to_the_expected_proto_commands() {
    let (link, dev) = setup();
    let (block, [up, def, rb], rs, pso, q) = record_compute(&dev, &link);
    let cmds = Decoder::new(&block).unwrap().collect_all().unwrap();
    let def_va = link_va(&link, def);
    let expect = vec![
        Command::BeginList(BeginList {
            list: ListId(1),
            queue: q,
            flags: 0,
            _pad: 0,
        }),
        // COMMON -> COPY_DEST emits nothing (implicit promotion).
        Command::CopyBufferRegion(CopyBufferRegion {
            dst: def,
            src: up,
            dst_offset: 0,
            src_offset: 0,
            size: 4096,
        }),
        Command::Barrier({
            let mut b = Barrier::memory(stage::BLIT, state::stage::SHADERS);
            b.res_before = def;
            b.res_after = def;
            b
        }),
        Command::SetRootSignature(SetRootSignature {
            bind_point: BindPoint::COMPUTE,
            root_sig: rs,
        }),
        Command::SetPipeline(SetPipeline { pso, _pad: 0 }),
        Command::SetRootConstants(SetRootConstants {
            bind_point: BindPoint::COMPUTE,
            root_index: 0,
            dest_offset: 0,
            data: &[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0],
        }),
        Command::SetRootDescriptor(SetRootDescriptor {
            bind_point: BindPoint::COMPUTE,
            root_index: 1,
            kind: RootParamKind::UAV,
            _pad: 0,
            gpu_va: def_va,
        }),
        Command::Dispatch(Dispatch {
            x: 4,
            y: 2,
            z: 1,
            _pad: 0,
        }),
        Command::Barrier({
            let mut b = Barrier::memory(state::stage::SHADERS, state::stage::SHADERS);
            b.res_before = def;
            b.res_after = def;
            b
        }),
        Command::Barrier({
            let mut b = Barrier::memory(state::stage::SHADERS, stage::BLIT);
            b.res_before = def;
            b.res_after = def;
            b
        }),
        Command::CopyBufferRegion(CopyBufferRegion {
            dst: rb,
            src: def,
            dst_offset: 0,
            src_offset: 0,
            size: 4096,
        }),
        Command::EndList(EndList {
            list: ListId(1),
            _pad: 0,
        }),
    ];
    assert_eq!(cmds, expect);
}

fn link_va(link: &MockLink, res: ResId) -> u64 {
    link.gpu_va(res).unwrap()
}

#[test]
fn golden_bytes() {
    let (link, dev) = setup();
    let (block, ..) = record_compute(&dev, &link);
    let path = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/compute-block.vmps.txt"
    ));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        hexfile::write(path, "compute-block.vmps", &block);
    }
    let golden = hexfile::read(path);
    assert_eq!(block, golden, "the stream the front writes changed");
}

#[test]
fn copy_round_trip_through_the_mock() {
    let (link, dev) = setup();
    let up = buf(
        &dev,
        resource::HEAP_UPLOAD,
        64,
        false,
        resource::STATE_GENERIC_READ,
    );
    let def = buf(&dev, resource::HEAP_DEFAULT, 64, false, 0);
    let rb = buf(
        &dev,
        resource::HEAP_READBACK,
        64,
        false,
        resource::STATE_COPY_DEST,
    );
    let q = dev.create_queue(0, 0).unwrap();
    let pattern: Vec<u8> = (0..64).collect();
    // SAFETY: the mock's upload buffer is 64 bytes of host memory.
    unsafe { std::slice::from_raw_parts_mut(up.map(0).unwrap() as *mut u8, 64) }
        .copy_from_slice(&pattern);
    let mut l = CommandList::new(&dev, 0);
    l.copy_buffer_region(&def, 0, &up, 0, 64);
    l.resource_barrier(&[ResourceBarrier::Transition {
        res: def.res,
        subresource: 0xffff_ffff,
        before: 0,
        after: state::COPY_SOURCE,
        flags: 0,
    }]);
    l.copy_buffer_region(&rb, 0, &def, 0, 64);
    q.execute(&[l.close().unwrap()]).unwrap();
    let fence = dev.create_fence(0).unwrap();
    q.signal(&fence, 1).unwrap();
    assert_eq!(fence.completed(), 1);
    assert_eq!(fence.wait(1, 10), Ok(true));
    assert_eq!(fence.wait(2, 10), Ok(false));
    assert_eq!(link.buffer_bytes(rb.res).unwrap(), pattern);
    assert!(link.rejected().is_empty());
    fence.signal(5).unwrap();
    assert_eq!(fence.completed(), 5);
    assert_eq!(link.fence_value(FenceId(fence.id.0)), Some(5));
}

#[test]
fn list_errors_surface_at_close_and_reset_clears_them() {
    let (_link, dev) = setup();
    let a = buf(&dev, resource::HEAP_DEFAULT, 16, false, 0);
    let b = buf(&dev, resource::HEAP_DEFAULT, 16, false, 0);
    let mut l = CommandList::new(&dev, 0);
    l.copy_buffer_region(&a, 8, &b, 0, 16);
    assert_eq!(l.close().err(), Some(Hr::E_INVALIDARG));
    // The second Close of a clean list is fine too.
    l.reset();
    l.copy_buffer_region(&a, 0, &b, 0, 16);
    assert!(l.close().is_ok());
    assert!(l.close().is_ok());
    // A transition on an unknown resource is an error as well.
    l.reset();
    l.resource_barrier(&[ResourceBarrier::Transition {
        res: ResId(9999),
        subresource: 0,
        before: 0,
        after: 1,
        flags: 0,
    }]);
    assert!(l.close().is_err());
    l.reset();
    l.unsupported();
    assert_eq!(l.close().err(), Some(Hr::E_NOTIMPL));
}

#[test]
fn split_barriers_send_at_end_only() {
    let (_link, dev) = setup();
    let b = buf(
        &dev,
        resource::HEAP_DEFAULT,
        16,
        true,
        state::UNORDERED_ACCESS,
    );
    let mut l = CommandList::new(&dev, 0);
    use ResourceBarrier::*;
    l.resource_barrier(&[Transition {
        res: b.res,
        subresource: 0xffff_ffff,
        before: state::UNORDERED_ACCESS,
        after: state::COPY_SOURCE,
        flags: state::BARRIER_FLAG_BEGIN_ONLY,
    }]);
    assert_eq!(l.len(), 0);
    l.resource_barrier(&[Transition {
        res: b.res,
        subresource: 0xffff_ffff,
        before: state::UNORDERED_ACCESS,
        after: state::COPY_SOURCE,
        flags: state::BARRIER_FLAG_END_ONLY,
    }]);
    assert_eq!(l.len(), 1);
}

#[test]
fn descriptor_table_and_heaps_resolve_to_proto_slots() {
    let (_link, dev) = setup();
    let heap = dev
        .create_descriptor_heap(&HeapDesc {
            ty: descriptor::TYPE_CBV_SRV_UAV,
            count: 16,
            flags: 1,
            node_mask: 0,
        })
        .unwrap();
    let hidden = dev
        .create_descriptor_heap(&HeapDesc {
            ty: descriptor::TYPE_CBV_SRV_UAV,
            count: 16,
            flags: 0,
            node_mask: 0,
        })
        .unwrap();
    let mut l = CommandList::new(&dev, 1);
    l.set_descriptor_heaps(&[&heap]);
    let inc = dev.descriptor_increment(descriptor::TYPE_CBV_SRV_UAV) as u64;
    l.set_compute_root_table(2, heap.gpu_start() + 5 * inc);
    assert!(l.close().is_ok());
    let items = l.close().unwrap();
    assert_eq!(
        items[0],
        Item::Cmd(Command::SetDescriptorHeaps(SetDescriptorHeaps {
            cbv_srv_uav: heap.back,
            sampler: HeapId::NULL
        }))
    );
    assert_eq!(
        items[1],
        Item::Cmd(Command::SetRootTable(SetRootTable {
            bind_point: BindPoint::COMPUTE,
            root_index: 2,
            heap: heap.back,
            index: 5
        }))
    );
    // Not shader visible, or a CPU handle as a GPU handle: refused at close.
    l.reset();
    l.set_descriptor_heaps(&[&hidden]);
    assert!(l.close().is_err());
    l.reset();
    l.set_compute_root_table(0, heap.cpu_start());
    assert!(l.close().is_err());
}

#[test]
fn a_submit_error_is_reported() {
    let (_link, dev) = setup();
    let q = dev.create_queue(0, 0).unwrap();
    let mut l = CommandList::new(&dev, 0);
    l.dispatch(1, 1, 1);
    let items = l.close().unwrap();
    q.execute(std::slice::from_ref(&items)).unwrap();
    drop(q);
    // The queue is gone in the back: a second one would fail.
    let ghost = dev.create_queue(0, 0).unwrap();
    drop(ghost);
}

#[test]
fn immediate_writes_name_buffers_by_address_and_reach_the_back_as_write_immediate() {
    let (link, dev) = setup();
    let a = buf(&dev, resource::HEAP_DEFAULT, 64, true, 0);
    let b = buf(&dev, resource::HEAP_DEFAULT, 64, true, 0);
    let rb = buf(
        &dev,
        resource::HEAP_READBACK,
        64,
        false,
        resource::STATE_COPY_DEST,
    );
    let q = dev.create_queue(0, 0).unwrap();
    let mut l = CommandList::new(&dev, 0);
    l.write_immediate(a.gpu_va + 8, 0xAABB_CCDD, 0);
    l.write_immediate(b.gpu_va + 60, 7, 2);
    l.copy_buffer_region(&rb, 0, &a, 0, 16);
    let items = l.close().unwrap();
    q.execute(&[items]).unwrap();
    let blocks = link.blocks();
    let cmds = Decoder::new(&blocks[0]).unwrap().collect_all().unwrap();
    let writes: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::WriteImmediate(w) => Some(*w),
            _ => None,
        })
        .collect();
    assert_eq!(writes.len(), 2);
    assert_eq!(
        (
            writes[0].dst,
            writes[0].offset,
            writes[0].size,
            writes[0].value
        ),
        (a.res, 8, 4, 0xAABB_CCDD)
    );
    assert_eq!(
        (writes[1].dst, writes[1].offset, writes[1].mode),
        (b.res, 60, WriteImmediate::MARKER_OUT)
    );
    assert!(link.rejected().is_empty());
    assert_eq!(
        &link.buffer_bytes(a.res).unwrap()[8..12],
        &0xAABB_CCDDu32.to_le_bytes()
    );
    // An address no buffer holds, one not aligned, one past the end, a bad mode: the list fails at Close.
    for (va, mode) in [
        (a.gpu_va + 62, 0),
        (a.gpu_va + 2, 0),
        (a.gpu_va + 64 + 0x10_0000, 0),
        (a.gpu_va, 3),
    ] {
        let mut l = CommandList::new(&dev, 0);
        l.write_immediate(va, 1, mode);
        assert_eq!(
            l.close().err(),
            Some(Hr::E_INVALIDARG),
            "va {va:#x} mode {mode}"
        );
    }
    // A dropped buffer's address no longer names anything.
    let gone = b.gpu_va;
    drop(b);
    let mut l = CommandList::new(&dev, 0);
    l.write_immediate(gone, 1, 0);
    assert_eq!(l.close().err(), Some(Hr::E_INVALIDARG));
}

#[test]
fn timestamps_rise_in_recording_order_and_resolve_into_the_buffer() {
    use victory_metal_front::timestamps::{HEAP_TYPE_TIMESTAMP, QueryHeap};
    let (link, dev) = setup();
    let dst = buf(&dev, resource::HEAP_DEFAULT, 64, true, 0);
    let rb = buf(
        &dev,
        resource::HEAP_READBACK,
        64,
        false,
        resource::STATE_COPY_DEST,
    );
    let heap = QueryHeap::new(HEAP_TYPE_TIMESTAMP, 4).unwrap();
    let q = dev.create_queue(0, 0).unwrap();
    let mut l = CommandList::new(&dev, 0);
    l.end_timestamp(&heap, 2);
    l.end_timestamp(&heap, 0);
    l.end_timestamp(&heap, 1);
    l.resolve_timestamps(&heap, 0, 3, &dst, 16);
    l.copy_buffer_region(&rb, 0, &dst, 0, 64);
    q.execute(&[l.close().unwrap()]).unwrap();
    let bytes = link.buffer_bytes(rb.res).unwrap();
    let t = |i: usize| u64::from_le_bytes(bytes[16 + 8 * i..24 + 8 * i].try_into().unwrap());
    assert!(t(2) < t(0) && t(0) < t(1), "{} {} {}", t(0), t(1), t(2));
    assert!(t(2) > 0);
    assert_eq!(&bytes[..16], &[0; 16], "nothing else is written");
    // Out of range, misaligned and out-of-buffer resolves fail the list.
    for (start, count, off) in [(2, 3, 0), (0, 1, 4), (0, 4, 40)] {
        let mut l = CommandList::new(&dev, 0);
        l.resolve_timestamps(&heap, start, count, &dst, off);
        assert_eq!(l.close().err(), Some(Hr::E_INVALIDARG));
    }
    let mut l = CommandList::new(&dev, 0);
    l.end_timestamp(&heap, 4);
    assert_eq!(l.close().err(), Some(Hr::E_INVALIDARG));
}

#[test]
fn pix_markers_become_debug_labels() {
    let (link, dev) = setup();
    let q = dev.create_queue(0, 0).unwrap();
    let mut l = CommandList::new(&dev, 0);
    l.marker('[', "Direct/Main #0");
    l.marker('=', "here");
    l.marker(']', "");
    q.execute(&[l.close().unwrap()]).unwrap();
    let blocks = link.blocks();
    let cmds = Decoder::new(&blocks[0]).unwrap().collect_all().unwrap();
    let labels: Vec<&[u8]> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::DebugLabel(t) => Some(*t),
            _ => None,
        })
        .collect();
    assert_eq!(labels, [&b"[Direct/Main #0"[..], b"=here", b"]"]);
}
