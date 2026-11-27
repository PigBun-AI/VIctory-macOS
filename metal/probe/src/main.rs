//! `m0-probe`: built for `x86_64-pc-windows-gnullvm` it runs under Wine and
//! calls the unixlib; built for darwin it runs the same measurements in
//! process, the baseline without Wine.
//!
//! Flags: `--quick` (small sizes), `--layer` (Wine only: the Metal-layer
//! test), `--hold-ms N` (keep the layer window up), `--which NAME.dll`
//! (Wine only; add `--system32` to load with LOAD_LIBRARY_SEARCH_SYSTEM32;
//! load that dll and say whether it is our stand-in, which
//! exports `victory_metal_bridge_anchor`; used for the DLL override test).

use victory_metal_probe::suite::{Config, run};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if let Some(i) = args.iter().position(|a| a == "--which") {
        let name = args.get(i + 1).map(String::as_str).unwrap_or("d3d12.dll");
        let system32 = args.iter().any(|a| a == "--system32");
        std::process::exit(which(name, system32));
    }
    let quick = args.iter().any(|a| a == "--quick");
    let cfg = if quick {
        Config::quick()
    } else {
        Config::full()
    };
    let report = platform_run(&args, &cfg);
    println!("\nchecks failed: {}", report.failures.len());
    std::process::exit(i32::from(!report.failures.is_empty()));
}

#[cfg(windows)]
fn platform_run(args: &[String], cfg: &Config) -> victory_metal_probe::suite::Report {
    use std::time::Duration;
    use victory_metal_bridge::pe::WineUnixlib;

    println!("m0-probe: PE under Wine, through the unixlib of victory_metal_bridge.dll");
    let link = match WineUnixlib::load("victory_metal_bridge.dll") {
        Ok(link) => link,
        Err(e) => {
            println!("CHECK  FAIL unixlib connected: {e}");
            let mut r = victory_metal_probe::suite::Report::default();
            r.failures.push(e);
            return r;
        }
    };
    println!("CHECK  PASS unixlib connected: module {:p}", link.module);
    let mut report = run(&link, cfg);
    if args.iter().any(|a| a == "--layer") {
        let hold = args
            .windows(2)
            .find(|w| w[0] == "--hold-ms")
            .and_then(|w| w[1].parse().ok())
            .unwrap_or(0);
        let layer = victory_metal_probe::layer::run(&link, Duration::from_millis(hold));
        report.failures.extend(layer.failures);
    }
    report
}

#[cfg(target_vendor = "apple")]
fn platform_run(_args: &[String], cfg: &Config) -> victory_metal_probe::suite::Report {
    use victory_metal_bridge::unix::InProcess;

    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    println!("m0-probe: native {arch} Mach-O, in process (no Wine)");
    run(&InProcess, cfg)
}

#[cfg(not(any(windows, target_vendor = "apple")))]
fn platform_run(_args: &[String], _cfg: &Config) -> victory_metal_probe::suite::Report {
    println!("m0-probe needs Windows (under Wine) or macOS");
    victory_metal_probe::suite::Report::default()
}

/// Loads `name` the way the game would and reports which file answered.
#[cfg(windows)]
fn which(name: &str, system32: bool) -> i32 {
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };

    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: Win32 loader calls with NUL-terminated names and a big enough buffer.
    unsafe {
        // `--system32` is how victory-gpu's root.rs loads d3d12.dll.
        let flags = if system32 {
            LOAD_LIBRARY_SEARCH_SYSTEM32
        } else {
            0
        };
        let module = LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), flags);
        if module.is_null() {
            println!("WHICH {name}: not loadable");
            return 2;
        }
        let mut path = [0u16; 260];
        let n = GetModuleFileNameW(module, path.as_mut_ptr(), 260) as usize;
        let ours = GetProcAddress(module, c"victory_metal_bridge_anchor".as_ptr().cast()).is_some();
        println!(
            "WHICH {name}: {} ({})",
            if ours {
                "OUR STAND-IN (native override applied)"
            } else {
                "Wine builtin (override not applied)"
            },
            String::from_utf16_lossy(&path[..n])
        );
        0
    }
}
