//! DXBC in: the runtime-compiled helper shaders of the game layer come out
//! of `D3DCompile` (shader model 5.x) as DXBC, and MSC only takes DXIL. The
//! Microsoft DXBC→DXIL converter (`libdxilconv`, the `dxilconv` project of
//! DirectXShaderCompiler: `projects/dxilconv/include/DxbcConverter.h`, read
//! 2026-11-27 from the `main` branch) is a COM component:
//!
//! ```text
//! DxcCreateInstance(CLSID_DxbcConverter 4900391E-B752-4EDD-A885-6FB76E25ADDB,
//!                   IID_IDxbcConverter  5F956ED5-78D1-4B15-8247-F7187614A041, &p)
//! p->Convert(dxbc, size, extra_options (wide, may be null),
//!            &dxil, &dxil_size, &diag (wide, may be null))
//! ```
//!
//! The vtable is `IUnknown` (3 slots) then `Convert`, `ConvertInDriver`. The
//! output buffer and the diagnostic are allocated with the component's
//! `CoTaskMemAlloc` (plain `malloc` in the non-Windows adapter layer) and
//! freed by the caller.
//!
//! The library is the one D3DMetal ships next to `libdxcompiler.dylib`
//! (x86_64 only: under an arm64 process the load fails and the error says
//! so). Search order: `VICTORY_DXILCONV_DYLIB` (only that, when set), then
//! the D3DMetal path of this repository's build tree, then the bare name.
//! It is loaded with `dlopen` at the first use, like MSC.
//!
//! One converter instance is created per call, so calls from several threads
//! are independent.

use std::ffi::{CStr, CString, c_void};
use std::sync::OnceLock;

use super::MscError;

/// Environment variable naming the library to load instead.
pub const DYLIB_ENV: &str = "VICTORY_DXILCONV_DYLIB";
/// Where this repository's build puts D3DMetal's copy, relative to the home
/// directory.
pub const DEFAULT_REL_PATH: &str = "Documents/GTA6Project/VIctory-macOS/target/wine-cx/lib/external/D3DMetal.framework/Versions/A/Resources/libdxilconv.dylib";

#[repr(C)]
struct Guid(u32, u16, u16, [u8; 8]);

const CLSID_DXBC_CONVERTER: Guid = Guid(
    0x4900_391e,
    0xb752,
    0x4edd,
    [0xa8, 0x85, 0x6f, 0xb7, 0x6e, 0x25, 0xad, 0xdb],
);
const IID_IDXBC_CONVERTER: Guid = Guid(
    0x5f95_6ed5,
    0x78d1,
    0x4b15,
    [0x82, 0x47, 0xf7, 0x18, 0x76, 0x14, 0xa0, 0x41],
);

type CreateInstance = unsafe extern "C" fn(*const Guid, *const Guid, *mut *mut c_void) -> i32;
type Release = unsafe extern "C" fn(*mut c_void) -> u32;
type ConvertFn = unsafe extern "C" fn(
    this: *mut c_void,
    dxbc: *const c_void,
    size: u32,
    options: *const u32,
    dxil: *mut *mut c_void,
    dxil_size: *mut u32,
    diag: *mut *mut u32,
) -> i32;

/// The `IDxbcConverter` vtable up to what is called.
#[repr(C)]
struct Vtbl {
    query_interface: *const c_void,
    add_ref: *const c_void,
    release: Release,
    convert: ConvertFn,
}

/// The loaded converter library.
#[derive(Clone, Copy)]
pub struct DxbcConverter {
    create: CreateInstance,
    path: &'static str,
}

// A function pointer into a library that is never unloaded.
unsafe impl Send for DxbcConverter {}
unsafe impl Sync for DxbcConverter {}

static LOADED: OnceLock<DxbcConverter> = OnceLock::new();

impl DxbcConverter {
    /// The converter, loading the library on the first call (a failure is not
    /// cached).
    pub fn get() -> Result<DxbcConverter, MscError> {
        if let Some(c) = LOADED.get() {
            return Ok(*c);
        }
        let c = DxbcConverter::load()?;
        Ok(*LOADED.get_or_init(|| c))
    }

    /// The paths [`DxbcConverter::load`] tries, in order.
    pub fn candidates() -> Vec<String> {
        match std::env::var(DYLIB_ENV) {
            Ok(p) if !p.is_empty() => vec![p],
            _ => {
                let mut v = Vec::new();
                if let Some(home) = std::env::var_os("HOME") {
                    v.push(format!("{}/{DEFAULT_REL_PATH}", home.to_string_lossy()));
                }
                v.push("libdxilconv.dylib".to_owned());
                v
            }
        }
    }

    /// Loads the first library that opens.
    pub fn load() -> Result<DxbcConverter, MscError> {
        DxbcConverter::load_from(&DxbcConverter::candidates())
    }

    /// Loads the first library of `paths` that opens.
    pub fn load_from(paths: &[String]) -> Result<DxbcConverter, MscError> {
        let tried = paths.to_vec();
        let mut details: Vec<String> = Vec::new();
        for path in &tried {
            let c = CString::new(path.as_str())
                .map_err(|_| MscError::BadInput(format!("{path:?} has a NUL")))?;
            // SAFETY: a NUL-terminated path; the handle is leaked on purpose.
            let h = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
            if h.is_null() {
                // SAFETY: null or a string valid until the next dl call.
                let e = unsafe { libc::dlerror() };
                details.push(format!(
                    "{path}: {}",
                    if e.is_null() {
                        "unknown dlopen error".to_owned()
                    } else {
                        // SAFETY: as above.
                        unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned()
                    }
                ));
                continue;
            }
            // SAFETY: a live handle, a NUL-terminated symbol name.
            let sym = unsafe { libc::dlsym(h, c"DxcCreateInstance".as_ptr()) };
            if sym.is_null() {
                details.push(format!("{path} has no DxcCreateInstance"));
                continue;
            }
            // SAFETY: the symbol has this signature (DxbcConverter.h).
            let create = unsafe { std::mem::transmute::<*mut c_void, CreateInstance>(sym) };
            return Ok(DxbcConverter {
                create,
                path: Box::leak(path.clone().into_boxed_str()),
            });
        }
        Err(MscError::Library {
            tried,
            detail: format!(
                "DXBC→DXIL converter (libdxilconv, x86_64 only) not available: {}; \
                 set {DYLIB_ENV} to D3DMetal's libdxilconv.dylib",
                details.join("; ")
            ),
        })
    }

    /// The path the library was loaded from.
    pub fn path(&self) -> &str {
        self.path
    }

    /// DXBC container in, DXIL container out. The diagnostic of a refusal is
    /// in the error.
    pub fn convert(&self, dxbc: &[u8]) -> Result<Vec<u8>, MscError> {
        let size =
            u32::try_from(dxbc.len()).map_err(|_| MscError::BadInput("DXBC over 4 GiB".into()))?;
        let mut p: *mut c_void = std::ptr::null_mut();
        // SAFETY: valid GUID pointers and an out pointer.
        let hr = unsafe { (self.create)(&CLSID_DXBC_CONVERTER, &IID_IDXBC_CONVERTER, &mut p) };
        if hr < 0 || p.is_null() {
            return Err(MscError::NoOutput(format!(
                "DxcCreateInstance(CLSID_DxbcConverter) failed: {hr:#010x}"
            )));
        }
        // SAFETY: a COM object: its first word points to the vtable.
        let vtbl = unsafe { &**(p as *const *const Vtbl) };
        let mut out: *mut c_void = std::ptr::null_mut();
        let mut out_size = 0u32;
        let mut diag: *mut u32 = std::ptr::null_mut();
        // SAFETY: a live converter; the input is a slice; outputs are out pointers.
        let hr = unsafe {
            (vtbl.convert)(
                p,
                dxbc.as_ptr().cast(),
                size,
                std::ptr::null(),
                &mut out,
                &mut out_size,
                &mut diag,
            )
        };
        let message = if diag.is_null() {
            String::new()
        } else {
            // The diagnostic is a wide string (4-byte `wchar_t` here).
            let mut s = String::new();
            for i in 0..4096 {
                // SAFETY: NUL-terminated, read stops at the NUL or the cap.
                let w = unsafe { *diag.add(i) };
                if w == 0 {
                    break;
                }
                s.push(char::from_u32(w).unwrap_or('?'));
            }
            // SAFETY: allocated by the component with malloc.
            unsafe { libc::free(diag.cast()) };
            s
        };
        let result = if hr < 0 || out.is_null() || out_size == 0 {
            Err(MscError::Unsupported(format!(
                "DXBC→DXIL conversion failed ({hr:#010x}){}",
                if message.is_empty() {
                    String::new()
                } else {
                    format!(": {}", message.trim())
                }
            )))
        } else {
            // SAFETY: the component returned `out_size` bytes at `out`.
            Ok(unsafe { std::slice::from_raw_parts(out.cast::<u8>(), out_size as usize) }.to_vec())
        };
        if !out.is_null() {
            // SAFETY: allocated by the component with malloc.
            unsafe { libc::free(out) };
        }
        // SAFETY: the one reference this function holds.
        unsafe { (vtbl.release)(p) };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_library_is_a_clear_error() {
        let e = DxbcConverter::load_from(&["/nonexistent/libdxilconv.dylib".to_owned()])
            .err()
            .unwrap();
        let text = e.to_string();
        assert!(matches!(e, MscError::Library { .. }));
        assert!(text.contains("/nonexistent/libdxilconv.dylib"), "{text}");
        assert!(text.contains(DYLIB_ENV), "{text}");
    }

    #[test]
    fn the_environment_variable_replaces_the_search() {
        // Not set here: the default list has the repository path and the bare name.
        let c = DxbcConverter::candidates();
        if std::env::var_os(DYLIB_ENV).is_none() {
            assert_eq!(c.last().unwrap(), "libdxilconv.dylib");
            assert!(c.len() >= 2 || std::env::var_os("HOME").is_none());
        } else {
            assert_eq!(c.len(), 1);
        }
    }
}
