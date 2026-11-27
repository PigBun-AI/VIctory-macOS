//! The bridge's Mach-O half with no setup call: the first proto call makes the
//! back's own `Executor`, and a copy round trip works on the real GPU through
//! the entries Wine calls (`__wine_unix_call_funcs`) and through `InProcess`.
#![cfg(target_vendor = "apple")]

use std::ffi::c_void;

use victory_metal_bridge::abi::*;
use victory_metal_bridge::unix::{
    __wine_unix_call_funcs, InProcess, clear_executor, use_default_executor,
};
use victory_metal_proto::cmd::*;
use victory_metal_proto::ids::*;
use victory_metal_proto::link::LinkExt;
use victory_metal_proto::stream::Encoder;
use victory_metal_proto::sync::*;
use victory_metal_proto::types::*;
use victory_metal_proto::{Caps, PROTO_VERSION, Status};

/// Calls through the table entry, as Wine does.
fn wine_call<A: SyncArgs>(a: &mut A) -> Status {
    a.head_mut().version = A::VERSION;
    let mut c = CallArgs {
        op: A::OP.0,
        _pad: 0,
        args: a as *mut A as u64,
        len: std::mem::size_of::<A>() as u64,
    };
    let nt = unsafe { __wine_unix_call_funcs[OP_PROTO_CALL as usize](&raw mut c as *mut c_void) };
    nt_to_status(nt)
}

#[test]
fn the_default_executor_runs_a_copy_round_trip() {
    use_default_executor();

    let mut hello = HelloArgs::new();
    hello.client_proto_version = PROTO_VERSION;
    hello.required_caps = Caps::METAL4;
    assert_eq!(wine_call(&mut hello), Status::OK, "HELLO through the table");
    assert!(hello.device_name[0] != 0);

    let mut mk = |kind: BufferKind, size: u64| {
        let mut a = CreateBufferArgs::new();
        a.size = size;
        a.kind = kind.0;
        assert_eq!(InProcess.call_args(&mut a), Status::OK);
        a
    };
    let up = mk(BufferKind::UPLOAD, 4096);
    let dflt = mk(BufferKind::DEFAULT, 4096);
    let rb = mk(BufferKind::READBACK, 4096);
    let q = {
        let mut a = CreateQueueArgs::new();
        a.kind = QueueKind::DIRECT.0;
        assert_eq!(InProcess.call_args(&mut a), Status::OK);
        a.queue
    };
    let f = {
        let mut a = CreateFenceArgs::new();
        assert_eq!(InProcess.call_args(&mut a), Status::OK);
        a.fence
    };

    let words: Vec<u32> = (0..1024).map(|i| i * 7 + 1).collect();
    unsafe { std::ptr::copy_nonoverlapping(words.as_ptr(), up.cpu_addr as *mut u32, 1024) };

    let copy = |dst: ResId, src: ResId| {
        Command::CopyBufferRegion(CopyBufferRegion {
            dst,
            src,
            dst_offset: 0,
            src_offset: 0,
            size: 4096,
        })
    };
    let mut e = Encoder::new(Caps::NONE);
    for c in [
        Command::BeginList(BeginList {
            list: ListId(1),
            queue: q,
            flags: 0,
            _pad: 0,
        }),
        copy(dflt.res, up.res),
        // COPY_DEST -> COPY_SOURCE: what the front sends for that transition.
        Command::Barrier(Barrier {
            class: BarrierClass::MEMORY,
            visibility: visibility::DEVICE,
            before: stage::BLIT,
            after: stage::BLIT,
            _pad: 0,
            res_before: ResId(0),
            res_after: ResId(0),
        }),
        copy(rb.res, dflt.res),
        Command::EndList(EndList {
            list: ListId(1),
            _pad: 0,
        }),
        Command::QueueSignal(QueueSignal {
            queue: q,
            fence: f,
            value: 1,
        }),
    ] {
        e.push(&c).unwrap();
    }
    let block = e.finish();
    let mut s = SubmitArgs {
        block: block.as_ptr() as u64,
        len: block.len() as u64,
    };
    let nt = unsafe { __wine_unix_call_funcs[OP_PROTO_SUBMIT as usize](&raw mut s as *mut c_void) };
    assert_eq!(nt_to_status(nt), Status::OK);

    let mut w = FenceWaitArgs::new();
    w.fence = f;
    w.value = 1;
    w.timeout_ms = 20_000;
    assert_eq!(wine_call(&mut w), Status::OK);

    let mut out = vec![0u32; 1024];
    unsafe { std::ptr::copy_nonoverlapping(rb.cpu_addr as *const u32, out.as_mut_ptr(), 1024) };
    assert!(out == words, "readback differs from what the upload buffer held");

    let mut sd = ShutdownArgs::new();
    assert_eq!(wine_call(&mut sd), Status::OK);
    clear_executor();
}
