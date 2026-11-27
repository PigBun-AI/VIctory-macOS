//! The helper shaders' compiler on Wine (docs/mac/PATCHES.md, G4).
//!
//! The Windows line compiles its own helper shaders (the indirect argument
//! clamp, the card compare, the sender, in-place, tiles, compose, ...) at run
//! time with `D3DCompile`. On Wine that is vkd3d-shader's `d3dcompiler_47`,
//! which has no methods on `RWByteAddressBuffer` (`Load`, `Store`,
//! `Interlocked*`), no stores to indexed `groupshared` arrays, no
//! `[RootSignature]` attribute (the root signature is then not in the
//! compiled shader, and `CreateRootSignature(0, bytes)` fails) and not
//! `[allow_uav_condition]`. The clamp is the one that hangs the title: without
//! it every indirect execution is dropped and the world load never ends.
//!
//! [`d3d_compile`], which the files that compile these shaders import in place
//! of `D3DCompile` (one line each), therefore sends the source to the DXC that
//! D3DMetal ships (`libdxcompiler.dylib`, x86_64: run under Rosetta by the small
//! front end `tools/macos/dxc-d3dmetal.sh` builds; `tools/mac/run.sh` builds it
//! and sets the variables). The shader model of the target is raised to 6.0, as
//! DXC has no older one. A result is kept in a file named by the hash of source,
//! entry and target, so the helper runs once per shader and machine.
//! Where it is not configured, fails, or off Wine, the call is `D3DCompile`'s.
//!
//! Variables: `VICTORY_DXC_HELPER` (Unix path of the front end),
//! `VICTORY_DXC_DIR` (Unix directory for sources, results, errors),
//! `DXC_LIB` (read by the front end), `VICTORY_MAC_HLSL=0` (off).

use std::path::PathBuf;

/// The settings, when the helper is configured and this is Wine on a Mac.
struct Dxc {
    helper: String,
    dir: String,
}

fn config() -> Option<&'static Dxc> {
    use std::sync::LazyLock;
    static CONFIG: LazyLock<Option<Dxc>> = LazyLock::new(|| {
        if !super::rosetta::rosetta() || std::env::var("VICTORY_MAC_HLSL").is_ok_and(|v| v == "0") {
            return None;
        }
        let helper = std::env::var("VICTORY_DXC_HELPER").ok().filter(|v| !v.is_empty())?;
        let dir = std::env::var("VICTORY_DXC_DIR").ok().filter(|v| !v.is_empty())?;
        std::fs::create_dir_all(unix_to_windows(&dir)).ok()?;
        Some(Dxc { helper, dir })
    });
    CONFIG.as_ref()
}

/// A Unix path as Wine's `Z:` drive sees it.
fn unix_to_windows(path: &str) -> PathBuf {
    PathBuf::from(format!("Z:{}", path.replace('/', "\\")))
}

/// `cs_5_1` is `cs_6_0` for DXC; a profile already 6.x stays.
pub fn dxc_profile(target: &str) -> String {
    match target.split_once('_') {
        Some((stage, rest)) if rest.starts_with('5') || rest.starts_with('4') => format!("{stage}_6_0"),
        _ => target.to_string(),
    }
}

/// A 64-bit hash (FNV-1a) of the parts, for file names.
fn key(parts: &[&[u8]]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for p in parts {
        for &b in *p {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The DXIL of `source` for `entry` and `target`, from the kept result or
/// from the helper, or why it could not be made.
#[cfg(windows)]
fn dxc(config: &Dxc, source: &[u8], entry: &str, target: &str) -> Result<Vec<u8>, String> {
    let profile = dxc_profile(target);
    let name = format!("{:016x}", key(&[source, entry.as_bytes(), profile.as_bytes()]));
    let (unix_dir, win_dir) = (&config.dir, unix_to_windows(&config.dir));
    let result = win_dir.join(format!("{name}.dxil"));
    if let Ok(kept) = std::fs::read(&result) {
        if !kept.is_empty() {
            return Ok(kept);
        }
    }
    let input = win_dir.join(format!("{name}.hlsl"));
    let errors = win_dir.join(format!("{name}.err"));
    std::fs::write(&input, source).map_err(|e| format!("cannot write {}: {e}", input.display()))?;
    let _ = std::fs::remove_file(&result);
    let started = std::time::Instant::now();
    // A Unix program started from Wine (through sh, to put its stderr in a
    // file: a handle passed to it by Wine is refused). Its exit status does
    // not come back through Wine, so the result file is the answer.
    let line = format!(
        "'{}' -T '{profile}' -E '{entry}' -Fo '{unix_dir}/{name}.dxil' '{unix_dir}/{name}.hlsl' 2>'{unix_dir}/{name}.err'; : >'{unix_dir}/{name}.done'",
        config.helper
    );
    let done = win_dir.join(format!("{name}.done"));
    let _ = std::fs::remove_file(&done);
    let status = run_unix(&format!("/bin/sh -c \"{line}\""), &done);
    let code = std::fs::read(&result).unwrap_or_default();
    if code.is_empty() {
        let text = std::fs::read_to_string(&errors).unwrap_or_default();
        return Err(format!("helper {status:?}: {}", text.trim()));
    }
    let _ = std::fs::remove_file(&input);
    let _ = std::fs::remove_file(&errors);
    let _ = std::fs::remove_file(&done);
    tracing::info!(
        "helper shader {name} ({entry}, {profile}) compiled by D3DMetal's DXC in {:.0} ms, {} bytes",
        started.elapsed().as_secs_f64() * 1e3,
        code.len()
    );
    Ok(code)
}

/// Starts the Unix program of `command_line` and waits for it to leave
/// `done` behind (Wine's process handle of a Unix program does not signal
/// when it ends). No handle is inherited: Wine refuses to pass them to it.
#[cfg(windows)]
fn run_unix(command_line: &str, done: &std::path::Path) -> Result<(), String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};
    let mut line: Vec<u16> = command_line.encode_utf16().chain([0]).collect();
    let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
    let mut info = PROCESS_INFORMATION::default();
    // SAFETY: a writable command line, valid structures, no inherited handles.
    unsafe {
        CreateProcessW(None, Some(windows::core::PWSTR(line.as_mut_ptr())), None, None, false, Default::default(), None, None, &startup, &mut info)
            .map_err(|e| format!("cannot start it: {e}"))?;
        let _ = CloseHandle(info.hProcess);
        let _ = CloseHandle(info.hThread);
    }
    let started = std::time::Instant::now();
    while !done.exists() {
        if started.elapsed() > std::time::Duration::from_secs(120) {
            return Err("it did not finish in 120 s".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Ok(())
}

/// `D3DCompile` of the Windows line's call sites, with DXC first where it is
/// configured (same signature; the sites import this in its place).
///
/// # Safety
/// As `D3DCompile`: `psrcdata` points to `srcdatasize` readable bytes and the
/// out pointers are valid.
#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
pub unsafe fn d3d_compile<P2, P4, P5, P6>(
    psrcdata: *const core::ffi::c_void,
    srcdatasize: usize,
    psourcename: P2,
    pdefines: Option<*const windows::Win32::Graphics::Direct3D::D3D_SHADER_MACRO>,
    pinclude: P4,
    pentrypoint: P5,
    ptarget: P6,
    flags1: u32,
    flags2: u32,
    ppcode: *mut Option<windows::Win32::Graphics::Direct3D::ID3DBlob>,
    pperrormsgs: Option<*mut Option<windows::Win32::Graphics::Direct3D::ID3DBlob>>,
) -> windows::core::Result<()>
where
    P2: windows::core::Param<windows::core::PCSTR>,
    P4: windows::core::Param<windows::Win32::Graphics::Direct3D::ID3DInclude>,
    P5: windows::core::Param<windows::core::PCSTR>,
    P6: windows::core::Param<windows::core::PCSTR>,
{
    use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCreateBlob};
    // SAFETY: PCSTRs from the caller (NUL-ended names, as D3DCompile needs).
    let (name, entry, target) = unsafe {
        (psourcename.param().abi(), pentrypoint.param().abi(), ptarget.param().abi())
    };
    if let Some(config) = config().filter(|_| pdefines.is_none()) {
        // SAFETY: as D3DCompile's own contract.
        let (text, entry_name, target_name) = unsafe {
            (
                std::slice::from_raw_parts(psrcdata.cast::<u8>(), srcdatasize),
                entry.to_string().unwrap_or_default(),
                target.to_string().unwrap_or_default(),
            )
        };
        match dxc(config, text, &entry_name, &target_name) {
            Ok(code) => {
                let made = unsafe {
                    D3DCreateBlob(code.len()).inspect(|blob| {
                        std::ptr::copy_nonoverlapping(code.as_ptr(), blob.GetBufferPointer().cast::<u8>(), code.len());
                    })
                };
                if let Ok(blob) = made {
                    // SAFETY: the caller's out pointer.
                    unsafe { *ppcode = Some(blob) };
                    return Ok(());
                }
            }
            Err(why) => {
                tracing::warn!("D3DMetal's DXC could not compile a helper shader ({entry_name}, {target_name}): {why}");
            }
        }
    }
    // SAFETY: the caller's contract, unchanged.
    let result = unsafe {
        D3DCompile(psrcdata, srcdatasize, name, pdefines, pinclude, entry, target, flags1, flags2, ppcode, pperrormsgs)
    };
    if let (Err(e), Some(errors)) = (&result, pperrormsgs) {
        // SAFETY: the caller's out pointer, just written by the call.
        if let Some(blob) = unsafe { (*errors).as_ref() } {
            // SAFETY: the blob's own bytes.
            let text = unsafe { std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()) };
            tracing::warn!("a helper shader does not compile on Wine's compiler ({e}): {}", String::from_utf8_lossy(text).trim());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shader_model_is_raised_to_6() {
        assert_eq!(dxc_profile("cs_5_1"), "cs_6_0");
        assert_eq!(dxc_profile("ps_5_0"), "ps_6_0");
        assert_eq!(dxc_profile("cs_6_6"), "cs_6_6");
        assert_eq!(key(&[b"a", b"b"]), key(&[b"a", b"b"]));
        assert_ne!(key(&[b"ab", b""]), key(&[b"a", b"b"]));
    }
}
