//! The guard in front of Metal Shader Converter (`msc/guard.rs`): a shader
//! with double precision, or one a census run named in
//! `VICTORY_METAL_MSC_DENYLIST`, is refused as `Unsupported` before MSC
//! runs. MSC 4.0.3 prints `[getMinMaxConstants:8246] Fatal: Unhandled LLVM
//! type for getMinMaxConstants` and aborts the process on a double-typed
//! wave reduction; without the guard the first test would take this test
//! process down with it.
//!
//! The DXIL comes from DXC (`common_msc.rs`; the tests skip without one).
//! The denylist test sets an environment variable, so it is the only test in
//! this file that reads `VICTORY_METAL_MSC_DENYLIST`, and the others do not
//! depend on it being unset: the variable is only ever set to a file whose
//! hash is of that test's own shader.
#![cfg(target_vendor = "apple")]

mod common_msc;

use victory_metal_back::Error;
use victory_metal_back::device::Device;
use victory_metal_back::msc::guard::{DENYLIST_ENV, stage_hash};
use victory_metal_back::msc::{
    CompileOptions, Msc, MscError, Param, ParamKind, RootSignatureDesc, Visibility, dxil,
};

use common_msc::dxil_or_skip;

/// The abort: a double-typed wave reduction.
const DOUBLE_WAVE: &str = "RWByteAddressBuffer o;\n\
    [numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
      double s = WaveActiveSum((double)t.x * 1.5);\n\
      o.Store(t.x*4, (uint)s);\n\
    }";
/// Double arithmetic MSC converts without complaint (it approximates): still
/// refused, the result would be wrong.
const DOUBLE_ARITH: &str = "RWByteAddressBuffer o;\n\
    [numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
      double a = (double)t.x * 1.5;\n\
      o.Store(t.x*4, (uint)(a * a + 0.25));\n\
    }";
/// The control: the same shape with floats.
const FLOAT_WAVE: &str = "RWByteAddressBuffer o;\n\
    [numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
      float s = WaveActiveSum((float)t.x * 1.5);\n\
      o.Store(t.x*4, (uint)s);\n\
    }";

fn root() -> RootSignatureDesc {
    RootSignatureDesc {
        flags: 0,
        params: vec![Param {
            kind: ParamKind::Uav {
                register: 0,
                space: 0,
            },
            visibility: Visibility::All,
        }],
        static_samplers: vec![],
    }
}

fn pipeline(d: &Device, dxil: &[u8]) -> Result<(), Error> {
    d.compute_pipeline_from_dxil(dxil, Some(&root()), &CompileOptions::default())
        .map(|_| ())
        .map_err(Error::from)
}

fn unsupported_with(e: &Error, text: &str) -> bool {
    matches!(e, Error::Unsupported(m) if m.contains(text))
}

#[test]
fn a_double_wave_sum_is_unsupported_and_the_process_lives() {
    let Some(code) = dxil_or_skip("f64_wave", "cs_6_6", DOUBLE_WAVE) else {
        return;
    };
    let info = dxil::inspect(&code).unwrap();
    assert_ne!(
        info.features.unwrap() & dxil::feature::ANY_DOUBLES,
        0,
        "DXC sets a double bit: {:#x}",
        info.features.unwrap()
    );
    let d = Device::new().expect("Metal 4 device");
    let e = pipeline(&d, &code).unwrap_err();
    assert!(
        unsupported_with(&e, "MSC 4.0.3 cannot convert double-precision"),
        "{e:?}"
    );
    // Through MSC's own entry as well.
    let e = Msc::get()
        .unwrap()
        .compile(&code, Some(&root()), &CompileOptions::default())
        .unwrap_err();
    assert!(matches!(e, MscError::Unsupported(_)), "{e:?}");
}

#[test]
fn double_arithmetic_is_unsupported_too() {
    let Some(code) = dxil_or_skip("f64_arith", "cs_6_6", DOUBLE_ARITH) else {
        return;
    };
    let d = Device::new().expect("Metal 4 device");
    let e = pipeline(&d, &code).unwrap_err();
    assert!(unsupported_with(&e, "double-precision"), "{e:?}");
}

#[test]
fn a_float_shader_of_the_same_shape_still_builds() {
    let Some(code) = dxil_or_skip("f32_wave", "cs_6_6", FLOAT_WAVE) else {
        return;
    };
    let info = dxil::inspect(&code).unwrap();
    assert_eq!(info.features.unwrap() & dxil::feature::ANY_DOUBLES, 0);
    let d = Device::new().expect("Metal 4 device");
    pipeline(&d, &code).expect("a shader without doubles is converted");
}

/// A container whose `SFI0` part is gone is asked for its types: the double
/// is found in the bitcode.
#[test]
fn without_a_feature_part_the_double_type_is_found_in_the_bitcode() {
    let Some(mut code) = dxil_or_skip("f64_nosfi", "cs_6_6", DOUBLE_WAVE) else {
        return;
    };
    let at = code
        .windows(4)
        .position(|w| w == b"SFI0")
        .expect("DXC writes SFI0");
    code[at..at + 4].copy_from_slice(b"XXXX");
    assert!(dxil::inspect(&code).unwrap().features.is_none());
    let d = Device::new().expect("Metal 4 device");
    let e = pipeline(&d, &code).unwrap_err();
    assert!(unsupported_with(&e, "double-precision"), "{e:?}");

    // And the float control is not mistaken for it (no MSC call needed).
    let Some(mut f) = dxil_or_skip("f32_nosfi", "cs_6_6", FLOAT_WAVE) else {
        return;
    };
    let at = f.windows(4).position(|w| w == b"SFI0").unwrap();
    f[at..at + 4].copy_from_slice(b"XXXX");
    let info = dxil::inspect(&f);
    assert!(!victory_metal_back::msc::guard::uses_doubles(
        &f,
        info.as_ref()
    ));
}

#[test]
fn a_denylisted_hash_is_unsupported_and_the_others_are_not() {
    let (Some(listed), Some(other)) = (
        dxil_or_skip("deny_listed", "cs_6_6", FLOAT_WAVE),
        dxil_or_skip(
            "deny_other",
            "cs_6_6",
            "RWByteAddressBuffer o;\n[numthreads(32,1,1)] void main(uint3 t : SV_DispatchThreadID) { o.Store(t.x*4, t.x + 1); }",
        ),
    ) else {
        return;
    };
    let d = Device::new().expect("Metal 4 device");
    // Before the list exists: builds.
    pipeline(&d, &listed).expect("builds without a denylist");

    let dir = std::env::temp_dir().join(format!("victory-denylist-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("msc-denylist.txt");
    std::fs::write(
        &file,
        format!(
            "# test\n{:016x}\t0000000000000000\tcensus: process abort\n",
            stage_hash(&listed)
        ),
    )
    .unwrap();
    // SAFETY: this is the only test of the file that touches the variable,
    // and the others are not affected by it (see the header).
    unsafe { std::env::set_var(DENYLIST_ENV, &file) };
    let e = pipeline(&d, &listed).unwrap_err();
    assert!(unsupported_with(&e, "census: MSC fatal"), "{e:?}");
    pipeline(&d, &other).expect("a hash not on the list builds");
    unsafe { std::env::remove_var(DENYLIST_ENV) };
    let _ = std::fs::remove_dir_all(&dir);
}
