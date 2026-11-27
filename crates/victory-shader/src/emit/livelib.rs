//! A translator in a dynamic library (`victory-shader-dylib`), for `VICTORY_LIVE_TRANSLATOR`:
//! the C ABI both sides agree on, and the host's side of it ([`Library`]).
//!
//! The library exports four functions (all `extern "C"`, names in [`symbols`]):
//!
//! * `victory_translator_abi() -> u32`: [`ABI_VERSION`] it was built for. The host refuses another.
//! * `victory_translator_info() -> *const c_char`: a NUL-terminated text for the log (name,
//!   version, build tag).
//! * `victory_translator_translate(container, len, settings, settings_len, host, out) -> i32`:
//!   the translation of one container under the given switches ([`settings_text`]: `NAME=value`
//!   lines, empty value = unset) and the four host numbers of [`super::host_settings`]; the answer
//!   is a [`Blob`] the library allocated, read with [`unpack`] and given back with
//!   `victory_translator_free(ptr, len)`. Status [`OK`]: the blob is the translated shader
//!   (`wire::encode`, magic `VSH1`, behind a [`HEADER`]); [`FAILED`]: its text is why the container
//!   does not translate; [`PANICKED`]: the translator panicked (text); [`BAD_CALL`]: arguments or
//!   ABI the library does not take.
//! * `victory_translator_free(ptr, len)`.
//!
//! Nothing but bytes, integers and pointers cross: the library has its own copy of every static of
//! the translator (switches, host numbers), so each call gives it the ones that matter. Rust types,
//! `tracing`'s subscriber and the allocator are not shared (the library's log lines go nowhere).

/// The version of the functions and the blob below. A change of either bumps it.
pub const ABI_VERSION: u32 = 1;

/// A blob starts with this, then [`ABI_VERSION`] as a little-endian `u32`.
pub const HEADER: &[u8; 4] = b"VTRB";

/// `victory_translator_translate` statuses.
pub const OK: i32 = 0;
pub const FAILED: i32 = 1;
pub const PANICKED: i32 = 2;
pub const BAD_CALL: i32 = 3;

/// The exported names.
pub mod symbols {
    pub const ABI: &[u8] = b"victory_translator_abi\0";
    pub const INFO: &[u8] = b"victory_translator_info\0";
    pub const TRANSLATE: &[u8] = b"victory_translator_translate\0";
    pub const FREE: &[u8] = b"victory_translator_free\0";
}

/// A buffer the library allocated.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Blob {
    pub ptr: *mut u8,
    pub len: usize,
}

impl Blob {
    pub const EMPTY: Blob = Blob { ptr: std::ptr::null_mut(), len: 0 };
}

pub type AbiFn = unsafe extern "C" fn() -> u32;
pub type InfoFn = unsafe extern "C" fn() -> *const std::ffi::c_char;
pub type TranslateFn = unsafe extern "C" fn(*const u8, usize, *const u8, usize, *const u32, *mut Blob) -> i32;
pub type FreeFn = unsafe extern "C" fn(*mut u8, usize);

/// The switches as the library is given them.
pub fn settings_text(settings: &[(String, String)]) -> String {
    settings.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

/// A blob's payload, after its header; the version it carries is checked.
pub fn unpack(blob: &[u8]) -> Result<&[u8], String> {
    let rest = blob.strip_prefix(HEADER.as_slice()).ok_or("the translator's answer has no header")?;
    let (version, payload) = rest.split_first_chunk::<4>().ok_or("the translator's answer is cut short")?;
    let version = u32::from_le_bytes(*version);
    if version != ABI_VERSION {
        return Err(format!("the translator's answer is version {version}, not {ABI_VERSION}"));
    }
    Ok(payload)
}

/// `payload` behind a header.
pub fn pack(payload: &[u8]) -> Vec<u8> {
    let mut out = HEADER.to_vec();
    out.extend(ABI_VERSION.to_le_bytes());
    out.extend(payload);
    out
}

#[cfg(windows)]
pub use client::Library;

#[cfg(windows)]
mod client {
    use super::*;
    use crate::emit::{HostShader, live, wire};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    /// A loaded translator library. Never unloaded: a pipeline or a callback of an older generation
    /// may still run its code.
    pub struct Library {
        pub path: PathBuf,
        /// The library's own description (`victory_translator_info`).
        pub info: String,
        translate: TranslateFn,
        free: FreeFn,
    }

    impl std::fmt::Debug for Library {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Library({}, {})", self.path.display(), self.info)
        }
    }

    // SAFETY: the library's functions take no state but their arguments (one translation per
    // call, thread-local scratch inside); they are called from any thread.
    unsafe impl Send for Library {}
    unsafe impl Sync for Library {}

    impl Library {
        /// Loads `path` and checks its ABI. The file stays loaded (and locked) for the run: load
        /// a copy, not the file a build writes.
        pub fn load(path: &Path) -> Result<Library, String> {
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: `wide` is a NUL-terminated path.
            let module = unsafe { LoadLibraryW(wide.as_ptr()) };
            if module.is_null() {
                return Err(format!("{}: is not a library this process can load (Windows error {})", path.display(), std::io::Error::last_os_error().raw_os_error().unwrap_or(0)));
            }
            let sym = |name: &[u8]| {
                // SAFETY: `name` is NUL-terminated, `module` is loaded.
                unsafe { GetProcAddress(module, name.as_ptr()) }
                    .ok_or_else(|| format!("{}: has no {}", path.display(), String::from_utf8_lossy(&name[..name.len() - 1])))
            };
            // SAFETY: the symbols are looked up by the names of the ABI; a library that exports
            // them with other signatures is not a translator library (and is refused by the
            // version first).
            let abi: AbiFn = unsafe { std::mem::transmute(sym(symbols::ABI)?) };
            let version = unsafe { abi() };
            if version != ABI_VERSION {
                return Err(format!("{}: ABI version {version}, this host speaks {ABI_VERSION}", path.display()));
            }
            let info: InfoFn = unsafe { std::mem::transmute(sym(symbols::INFO)?) };
            let translate: TranslateFn = unsafe { std::mem::transmute(sym(symbols::TRANSLATE)?) };
            let free: FreeFn = unsafe { std::mem::transmute(sym(symbols::FREE)?) };
            let text = unsafe { info() };
            let info = if text.is_null() {
                String::new()
            } else {
                // SAFETY: NUL-terminated static text of the library.
                unsafe { std::ffi::CStr::from_ptr(text) }.to_string_lossy().into_owned()
            };
            Ok(Library { path: path.to_owned(), info, translate, free })
        }

        /// The translation of `container` under `snapshot`'s switches and the host numbers `host`
        /// ([`crate::emit::host_settings`]).
        pub fn translate(&self, container: &[u8], snapshot: &live::Snapshot, host: [u32; 4]) -> Result<HostShader, String> {
            let settings = settings_text(&snapshot.settings());
            let mut out = Blob::EMPTY;
            // SAFETY: pointers and lengths of live slices; `out` is written by the library.
            let status = unsafe {
                (self.translate)(container.as_ptr(), container.len(), settings.as_ptr(), settings.len(), host.as_ptr(), &mut out)
            };
            let bytes = if out.ptr.is_null() {
                Vec::new()
            } else {
                // SAFETY: the library's buffer of `len` bytes, until it is given back.
                let b = unsafe { std::slice::from_raw_parts(out.ptr, out.len) }.to_vec();
                unsafe { (self.free)(out.ptr, out.len) };
                b
            };
            let payload = unpack(&bytes)?;
            match status {
                OK => wire::decode(payload).ok_or_else(|| "the translator's shader does not decode".to_owned()),
                FAILED => Err(String::from_utf8_lossy(payload).into_owned()),
                PANICKED => Err(format!("the translator library panicked: {}", String::from_utf8_lossy(payload))),
                other => Err(format!("the translator library refused the call (status {other}): {}", String::from_utf8_lossy(payload))),
            }
        }
    }
}
