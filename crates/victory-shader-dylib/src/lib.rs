//! The translator behind the C ABI of `victory_shader::emit::livelib`, built as a DLL
//! (`cargo build --release -p victory-shader-dylib`) for `VICTORY_LIVE_TRANSLATOR`: the running
//! game loads it, and loads the next one when the file changes (`victory-gpu`'s `live`).
//!
//! This crate holds no translation code. It links `victory-shader` and calls its `translate`;
//! a change to the translator shows in the DLL on the next build.

use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};

use victory_shader::emit::livelib::{ABI_VERSION, BAD_CALL, Blob, FAILED, OK, PANICKED, pack};
use victory_shader::emit::{live, wire};

/// The build tag (`VICTORY_DYLIB_TAG` when the DLL was built; a test of the swap sets it to tell two
/// builds of the same code apart).
const TAG: &str = match option_env!("VICTORY_DYLIB_TAG") {
    Some(t) => t,
    None => "-",
};

/// Text for the host's log: crate, version, ABI and the build tag.
pub fn info() -> String {
    format!("victory-shader-dylib {} abi {ABI_VERSION} tag={TAG}", env!("CARGO_PKG_VERSION"))
}

#[unsafe(no_mangle)]
pub extern "C" fn victory_translator_abi() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn victory_translator_info() -> *const c_char {
    static TEXT: std::sync::OnceLock<std::ffi::CString> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| std::ffi::CString::new(info()).unwrap_or_default()).as_ptr()
}

/// The switches of `text` (`NAME=value` lines; an empty value is unset). A name this build does not
/// know is skipped: the host may be newer than the DLL or the other way round.
fn settings(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| live::WHITELIST.contains(k))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// One translation: the status and the blob's payload (the shader, or the reason it failed).
pub fn run(container: &[u8], settings_text: &str, host: [u32; 4]) -> (i32, Vec<u8>) {
    let snapshot = match live::snapshot_of(&settings(settings_text)) {
        Ok(s) => s,
        Err(e) => return (BAD_CALL, e.into_bytes()),
    };
    // The host's numbers (this copy of the translator has statics of its own).
    victory_shader::emit::set_host_max_wave(host[0]);
    victory_shader::emit::set_view_heap_limit(host[1]);
    victory_shader::emit::set_raw_views(host[2]);
    victory_shader::emit::set_host_rdna(host[3] & 0x1FFF_FFFF != 0); // mac S1: bit 31 is the capability; mac S5: bits 29, 30 the mesh-compute mode
    victory_shader::emit::set_host_attribute_at_vertex(host[3] >> 31 == 0); // mac S1
    victory_shader::emit::set_host_mesh_compute(match host[3] >> 29 & 3 { 1 => victory_shader::emit::mesh_cs::MeshComputeMode::Problem, 2 => victory_shader::emit::mesh_cs::MeshComputeMode::All, _ => victory_shader::emit::mesh_cs::MeshComputeMode::Off }); // mac S5
    let done = catch_unwind(AssertUnwindSafe(|| live::scoped(&snapshot, || victory_shader::emit::translate(container))));
    match done {
        Ok(Ok(shader)) => (OK, wire::encode(&shader)),
        Ok(Err(e)) => (FAILED, e.to_string().into_bytes()),
        Err(panic) => {
            let why = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "(no message)".into());
            (PANICKED, why.into_bytes())
        }
    }
}

/// # Safety
/// `container` and `settings` point to that many readable bytes, `host` to four `u32`, `out` to a
/// writable [`Blob`]; the blob it gets goes back to [`victory_translator_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn victory_translator_translate(
    container: *const u8,
    len: usize,
    settings: *const u8,
    settings_len: usize,
    host: *const u32,
    out: *mut Blob,
) -> i32 {
    if out.is_null() {
        return BAD_CALL;
    }
    // SAFETY: as documented.
    unsafe { *out = Blob::EMPTY };
    let (status, payload) = if container.is_null() || settings.is_null() || host.is_null() {
        (BAD_CALL, b"null argument".to_vec())
    } else {
        // SAFETY: as documented.
        let (container, text, host) = unsafe {
            (
                std::slice::from_raw_parts(container, len),
                std::slice::from_raw_parts(settings, settings_len),
                [*host, *host.add(1), *host.add(2), *host.add(3)],
            )
        };
        match std::str::from_utf8(text) {
            Ok(text) => run(container, text, host),
            Err(_) => (BAD_CALL, b"settings are not UTF-8".to_vec()),
        }
    };
    let boxed = pack(&payload).into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed).cast::<u8>();
    // SAFETY: as documented.
    unsafe { *out = Blob { ptr, len } };
    status
}

/// # Safety
/// `ptr` and `len` are a blob [`victory_translator_translate`] gave.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn victory_translator_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        // SAFETY: made by `Box::into_raw` of a boxed slice of `len` bytes above.
        drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)) });
    }
}
