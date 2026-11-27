//! Helpers of the MSC tests: HLSL to DXIL through the DXC front end the
//! repository already builds for D3DMetal's `libdxcompiler.dylib`
//! (`tools/macos/dxc-d3dmetal.sh`; docs/macos.md, "VICTORY_HELPER_DXC").
//!
//! `VICTORY_HELPER_DXC` names the built front end and `DXC_LIB` the
//! library. The library is x86_64 only, so the front end runs under
//! Rosetta (`arch -x86_64`), also from an arm64 test process. Without both
//! variables the DXIL tests print `SKIPPED` and pass: no DXIL is stored in
//! the repository (asset policy), and no DXC ships with macOS.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

/// DXIL of `hlsl` for `profile` (e.g. `cs_6_6`), or `None` when no DXC is
/// configured.
pub fn dxil(name: &str, profile: &str, hlsl: &str) -> Option<Vec<u8>> {
    let helper = std::env::var_os("VICTORY_HELPER_DXC")?;
    std::env::var_os("DXC_LIB")?;
    let dir: PathBuf = std::env::temp_dir().join(format!("victory-msc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join(format!("{name}.hlsl"));
    let out = dir.join(format!("{name}.dxil"));
    std::fs::write(&src, hlsl).unwrap();
    let status = Command::new("/usr/bin/arch")
        .arg("-x86_64")
        .arg(helper)
        .args(["-T", profile, "-E", "main", "-Fo"])
        .arg(&out)
        .arg(&src)
        .status()
        .expect("run the DXC front end");
    assert!(status.success(), "DXC refused {name}");
    let bytes = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    Some(bytes)
}

/// The DXIL, or a `SKIPPED` line and `None`.
pub fn dxil_or_skip(name: &str, profile: &str, hlsl: &str) -> Option<Vec<u8>> {
    let d = dxil(name, profile, hlsl);
    if d.is_none() {
        eprintln!("SKIPPED {name}: set VICTORY_HELPER_DXC and DXC_LIB (docs/metal/msc.md)");
    }
    d
}
