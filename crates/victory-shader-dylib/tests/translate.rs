//! The DLL's C ABI, and the DLL against the translator linked into this test: the same bytes.
//!
//! Containers: `*.cdxil` files (a game run's `VICTORY_SHADER_SAVE=<dir>` writes them; `VICTORY_LIVE_CORPUS`
//! names the folder, or several separated like `PATH`; unset, the tests that need containers are skipped).
//! The DLL is the one `cargo build -p victory-shader-dylib` (same profile as this test) wrote next
//! to this test's `deps` folder.

#![cfg(windows)]

use std::path::{Path, PathBuf};

use victory_shader::emit::livelib::{self, ABI_VERSION, BAD_CALL, Blob, FAILED, Library, OK};
use victory_shader::emit::{host_settings, live, wire};
use victory_shader_dylib as dylib;

/// The tests set the translator's host numbers, which are process-wide: one at a time.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn corpus() -> Vec<(String, Vec<u8>)> {
    let dirs: Vec<PathBuf> = std::env::var_os("VICTORY_LIVE_CORPUS").map(|v| std::env::split_paths(&v).collect()).unwrap_or_default();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "cdxil")).collect();
        files.sort();
        for p in files {
            let Ok(bytes) = std::fs::read(&p) else { continue };
            if seen.insert(blake3::hash(&bytes)) {
                out.push((p.file_name().unwrap().to_string_lossy().into_owned(), bytes));
            }
        }
    }
    out.truncate(
        std::env::var("VICTORY_LIVE_CORPUS_MAX").ok().and_then(|n| n.parse().ok()).unwrap_or(60),
    );
    out
}

fn dll_path() -> PathBuf {
    // `cargo test` writes the DLL beside the test (target/<profile>/deps); `cargo build` also puts
    // one in target/<profile>.
    let exe = std::env::current_exe().unwrap();
    let deps = exe.parent().unwrap();
    [deps.join("victory_shader_dylib.dll"), deps.parent().unwrap().join("victory_shader_dylib.dll")]
        .into_iter()
        .find(|p| p.exists())
        .unwrap_or_else(|| panic!("no victory_shader_dylib.dll beside {}: run `cargo build -p victory-shader-dylib` first", deps.display()))
}
/// The exports called as the host calls them, in this process.
fn call(container: &[u8], settings: &str, host: [u32; 4]) -> (i32, Vec<u8>) {
    let mut out = Blob::EMPTY;
    // SAFETY: slices and a four-word array live through the call.
    let status = unsafe {
        dylib::victory_translator_translate(container.as_ptr(), container.len(), settings.as_ptr(), settings.len(), host.as_ptr(), &mut out)
    };
    assert!(!out.ptr.is_null(), "an answer is always given back");
    // SAFETY: the blob the call gave, until freed.
    let bytes = unsafe { std::slice::from_raw_parts(out.ptr, out.len) }.to_vec();
    unsafe { dylib::victory_translator_free(out.ptr, out.len) };
    (status, livelib::unpack(&bytes).unwrap().to_vec())
}

#[test]
fn abi_round_trip() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(dylib::victory_translator_abi(), ABI_VERSION);
    // SAFETY: the info text is NUL-terminated static.
    let info = unsafe { std::ffi::CStr::from_ptr(dylib::victory_translator_info()) }.to_string_lossy().into_owned();
    assert!(info.starts_with("victory-shader-dylib "), "{info}");

    // A container that does not translate: the same reason as the linked translator gives.
    let junk = b"not a container";
    let (status, why) = call(junk, "", [0; 4]);
    assert_eq!(status, FAILED);
    let linked = live::scoped(&live::snapshot_of(&[]).unwrap(), || victory_shader::emit::translate(junk)).unwrap_err();
    assert_eq!(String::from_utf8(why).unwrap(), linked.to_string());

    // Nothing in, nothing out: refused, not a crash; the out blob is set either way.
    let mut out = Blob { ptr: 1 as *mut u8, len: 9 };
    // SAFETY: null inputs are the case under test; `out` is writable.
    let status = unsafe { dylib::victory_translator_translate(std::ptr::null(), 0, std::ptr::null(), 0, std::ptr::null(), &mut out) };
    assert_eq!(status, BAD_CALL);
    unsafe { dylib::victory_translator_free(out.ptr, out.len) };
    // SAFETY: a null out is refused.
    assert_eq!(unsafe { dylib::victory_translator_translate(junk.as_ptr(), 1, junk.as_ptr(), 0, [0u32; 4].as_ptr(), std::ptr::null_mut()) }, BAD_CALL);

    // Settings in text: a name the DLL does not know is skipped, a known one is read.
    let (status, _) = call(junk, "VICTORY_NOT_A_SWITCH=1\nVICTORY_RT_PAGECACHE=1\n\n", [0; 4]);
    assert_eq!(status, FAILED);

    // Blob framing: a wrong version or header is refused, a good one gives its payload back.
    let packed = livelib::pack(b"abc");
    assert_eq!(livelib::unpack(&packed).unwrap(), b"abc");
    let mut bad = packed.clone();
    bad[4] ^= 1;
    assert!(livelib::unpack(&bad).is_err());
    assert!(livelib::unpack(b"XXXX\x01\0\0\0abc").is_err());
    assert!(livelib::unpack(b"VTR").is_err());

    // A real container, through the exports: decodes, and is what the linked translator makes.
    for (name, bytes) in corpus().into_iter().take(3) {
        let (status, payload) = call(&bytes, "", host_settings());
        let linked = live::scoped(&live::snapshot_of(&[]).unwrap(), || victory_shader::emit::translate(&bytes));
        match linked {
            Ok(shader) => {
                assert_eq!(status, OK, "{name}");
                assert_eq!(wire::decode(&payload).as_ref(), Some(&shader), "{name}");
                assert_eq!(payload, wire::encode(&shader), "{name}: bytes");
            }
            Err(e) => assert_eq!((status, String::from_utf8(payload).unwrap()), (FAILED, e.to_string()), "{name}"),
        }
    }
}

/// The loaded DLL (the host's `Library`) and the linked translator, same containers, same
/// switches, same host numbers: equal results (translated shaders and failure reasons alike).
#[test]
fn dll_and_linked_translator_agree_byte_for_byte() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let corpus = corpus();
    if corpus.is_empty() {
        eprintln!("skipped: no containers; set VICTORY_LIVE_CORPUS to a folder of .cdxil");
        return;
    }
    // The host's numbers, set for the linked copy (the DLL is given them per call).
    victory_shader::emit::set_host_max_wave(32);
    victory_shader::emit::set_view_heap_limit(999_999);
    victory_shader::emit::set_raw_views(0);
    let host = host_settings();
    assert_eq!(host[0], 32);
    // The DLL is loaded from a copy, as the host does (the build's file stays free).
    let copy = std::env::temp_dir().join(format!("victory-dylib-test-{}.dll", std::process::id()));
    std::fs::copy(dll_path(), &copy).unwrap();
    let lib = Library::load(Path::new(&copy)).unwrap();
    assert!(lib.info.starts_with("victory-shader-dylib "), "{}", lib.info);

    let variants: [&[(&str, &str)]; 4] = [&[], &[("VICTORY_RT_PAGECACHE", "1")], &[("VICTORY_SHADER_OPT", "0"), ("VICTORY_WAVE_WIDE", "0")], &[("VICTORY_VIEWADDR_TABLE", "1")]];
    let (mut same, mut failed_same) = (0, 0);
    for settings in variants {
        let settings: Vec<(String, String)> = settings.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        let snap = live::snapshot_of(&settings).unwrap();
        for (name, bytes) in &corpus {
            let linked = live::scoped(&snap, || victory_shader::emit::translate(bytes)).map_err(|e| e.to_string());
            let dll = lib.translate(bytes, &snap, host);
            match (&linked, &dll) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(a.dxil, b.dxil, "{name} {settings:?}: dxil bytes differ");
                    assert_eq!(a, b, "{name} {settings:?}: metadata differs");
                    same += 1;
                }
                (Err(a), Err(b)) => {
                    assert_eq!(a, b, "{name} {settings:?}");
                    failed_same += 1;
                }
                _ => panic!("{name} {settings:?}: one translated, the other not: {:?} / {:?}", linked.as_ref().map(|_| ()), dll.as_ref().map(|_| ())),
            }
        }
    }
    println!("dll == linked: {same} translations byte-identical, {failed_same} identical failures, {} containers x {} switch sets", corpus.len(), variants.len());
    assert!(same > 0, "nothing translated at all: the comparison proved nothing");
    // The page-cache switch changes something where the program has page loads: the variants are not all one result.
    // (Not asserted per container; the equality above is the test.)
}
