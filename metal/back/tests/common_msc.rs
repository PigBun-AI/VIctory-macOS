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
    match dxil_result(name, profile, hlsl)? {
        Ok(b) => Some(b),
        Err(e) => panic!("DXC refused {name}: {e}"),
    }
}

/// As [`dxil`], with DXC's refusal as a value (`Err(its message)`).
pub fn dxil_result(name: &str, profile: &str, hlsl: &str) -> Option<Result<Vec<u8>, String>> {
    let helper = std::env::var_os("VICTORY_HELPER_DXC")?;
    std::env::var_os("DXC_LIB")?;
    // Tests run in parallel and may reuse a name: the folder is unique per call.
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir: PathBuf =
        std::env::temp_dir().join(format!("victory-msc-{}-{n}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join(format!("{name}.hlsl"));
    let out = dir.join(format!("{name}.dxil"));
    std::fs::write(&src, hlsl).unwrap();
    let run = Command::new("/usr/bin/arch")
        .arg("-x86_64")
        .arg(helper)
        .args(["-T", profile, "-E", "main", "-Fo"])
        .arg(&out)
        .arg(&src)
        .output()
        .expect("run the DXC front end");
    let result = if run.status.success() {
        Ok(std::fs::read(&out).unwrap())
    } else {
        Err(String::from_utf8_lossy(&run.stderr)
            .lines()
            .find(|l| l.contains("error"))
            .unwrap_or("compilation failed")
            .to_owned())
    };
    let _ = std::fs::remove_dir_all(&dir);
    Some(result)
}

/// The DXIL, or a `SKIPPED` line and `None`.
pub fn dxil_or_skip(name: &str, profile: &str, hlsl: &str) -> Option<Vec<u8>> {
    let d = dxil(name, profile, hlsl);
    if d.is_none() {
        eprintln!("SKIPPED {name}: set VICTORY_HELPER_DXC and DXC_LIB (docs/metal/msc.md)");
    }
    d
}

/// DXBC of `hlsl` for `profile` (e.g. `cs_5_1`) from D3DCompile under Wine
/// (`tools/metal/dxbc-compile.sh`, named by `VICTORY_DXBC_TOOL`), or `None`
/// when that is not configured. The converter that reads it
/// (`libdxilconv`) is x86_64 only, so a test using this also needs the
/// x86_64 process (Rosetta) and prints SKIPPED on arm64.
pub fn dxbc_or_skip(name: &str, profile: &str, hlsl: &str) -> Option<Vec<u8>> {
    let Some(tool) = std::env::var_os("VICTORY_DXBC_TOOL") else {
        eprintln!("SKIPPED {name}: VICTORY_DXBC_TOOL (tools/metal/dxbc-compile.sh) is not set");
        return None;
    };
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir: PathBuf =
        std::env::temp_dir().join(format!("victory-dxbc-{}-{n}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join(format!("{name}.hlsl"));
    let out = dir.join(format!("{name}.dxbc"));
    std::fs::write(&src, hlsl).unwrap();
    let run = Command::new(tool)
        .arg(&src)
        .arg(&out)
        .arg(profile)
        .output()
        .expect("run the DXBC tool");
    assert!(
        run.status.success() && out.exists(),
        "D3DCompile refused {name}: {}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    Some(std::fs::read(&out).unwrap())
}
