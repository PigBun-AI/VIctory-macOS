//! A machine without Metal Shader Converter: the interpreter keeps working
//! (copies, root signatures with the rule-based layout) and
//! `CREATE_COMPUTE_PIPELINE` answers `UNSUPPORTED` with the reason kept in
//! `Executor::last_error`, never a panic. Its own test binary: the library
//! path is set before anything loads MSC.
#![cfg(target_vendor = "apple")]

mod exec_support;

use exec_support::*;
use victory_metal_proto::root::*;
use victory_metal_proto::status::Status;
use victory_metal_proto::sync::CreateComputePipelineArgs;
use victory_metal_proto::types::*;

#[test]
fn without_msc_pipelines_are_unsupported_and_the_rest_works() {
    // SAFETY: the only test of this binary, before any thread of its own.
    unsafe {
        std::env::set_var(
            "VICTORY_MSC_DYLIB",
            "/nonexistent/libmetalirconverter.dylib",
        )
    };
    let fx = Fx::new();
    let sig = fx.root_sig(&RootSigDesc {
        params: vec![constants_param(3), descriptor_param(RootParamKind::UAV)],
        ..RootSigDesc::default()
    });
    let l = fx.executor().tlab_layout(sig).unwrap();
    assert!(!l.from_reflection_data(), "the rule-based layout stands in");
    assert_eq!((l.slot(1).unwrap().offset, l.size()), (16, 24));
    let bytes = [0x44u8, 0x58, 0x42, 0x43, 0, 0, 0, 0];
    let mut a = CreateComputePipelineArgs::new();
    a.root_sig = sig;
    a.dxil_addr = bytes.as_ptr() as u64;
    a.dxil_len = bytes.len() as u64;
    assert_eq!(fx.status(a), Status::UNSUPPORTED);
    let why = fx.executor().last_error().expect("a reason");
    assert!(
        why.contains("/nonexistent/libmetalirconverter.dylib"),
        "{why}"
    );
    // The device still works.
    let q = fx.queue();
    let src = fx.buffer(BufferKind::SHARED, 64);
    let dst = fx.buffer(BufferKind::SHARED, 64);
    cpu_write(src.cpu_addr, &[1, 2, 3, 4]);
    let f = fx.fence(0);
    let cmds = vec![
        begin(1, q),
        copy(dst.res, src.res, 16),
        end(1),
        signal_cmd(q, f, 1),
    ];
    assert_eq!(fx.submit(&cmds), Status::OK);
    assert_eq!(fx.wait(f, 1), Status::OK);
    assert_eq!(cpu_read(dst.cpu_addr, 4), [1, 2, 3, 4]);
}
