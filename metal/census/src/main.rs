//! `victory-census`: every shader program of the game package through the
//! whole chain, offline (Xbox ISA -> DXIL -> Metal Shader Converter ->
//! MTL4Compiler), with failure classes, timings and a pipeline archive.
//!
//! ```text
//! victory-census translate  [--game DIR] [--out DIR] [--jobs N] [--limit N]
//! victory-census convert    [--out DIR] [--kind cs|vs|ps|ms|gs|all] [--jobs N] [--archive]
//! victory-census archive    [--out DIR] ...          cold compile against archive load
//! victory-census report     [--out DIR] [--research DIR]
//! victory-census opcodes    [--game DIR] [--out DIR] [--research DIR] [--jobs N] [--limit N]
//!                           GFX10 instruction counts per mnemonic and class (opcodes.jsonl,
//!                           and opcodes.json/.md under --research)
//! ```
//!
//! All game-derived data (translated DXIL, metallibs, archives, per-shader
//! rows with names) stays under `--out` (default
//! `~/Documents/GTA6Project/victory-data/census`), outside the repository.
//! The tool only reads the game directory. See `docs/metal/census.md`.

#[cfg(target_vendor = "apple")]
mod convert;
#[cfg(target_vendor = "apple")]
mod defaults;
#[cfg(target_vendor = "apple")]
mod dxc;
#[cfg(target_vendor = "apple")]
mod jsonl;
#[cfg(target_vendor = "apple")]
mod meshfit;
#[cfg(target_vendor = "apple")]
mod meshstat;
#[cfg(target_vendor = "apple")]
mod opcodes;
#[cfg(target_vendor = "apple")]
mod report;
#[cfg(target_vendor = "apple")]
mod root;
#[cfg(target_vendor = "apple")]
mod sig;
#[cfg(target_vendor = "apple")]
mod stats;
#[cfg(target_vendor = "apple")]
mod translate;

#[cfg(target_vendor = "apple")]
use std::path::PathBuf;

/// `--name value` from the arguments.
#[cfg(target_vendor = "apple")]
pub fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[cfg(target_vendor = "apple")]
pub fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// The project root (`GTA6_ROOT`, else `~/Documents/GTA6Project`).
#[cfg(target_vendor = "apple")]
pub fn project_root() -> PathBuf {
    if let Some(r) = std::env::var_os("GTA6_ROOT") {
        return PathBuf::from(r);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Documents").join("GTA6Project")
}

#[cfg(target_vendor = "apple")]
pub fn out_dir(args: &[String]) -> PathBuf {
    arg(args, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root().join("victory-data").join("census"))
}

#[cfg(target_vendor = "apple")]
pub fn jobs(args: &[String]) -> usize {
    arg(args, "--jobs")
        .and_then(|j| j.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()))
}

#[cfg(target_vendor = "apple")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!(
            "usage: victory-census translate|convert|archive|report [options] (see the module docs)"
        );
        std::process::exit(2);
    };
    let rest = &args[1..];
    let result = match cmd.as_str() {
        "translate" => translate::run(rest),
        "convert" => convert::run(rest),
        "report" => report::run(rest),
        "meshstat" => meshstat::run(rest),
        "meshfit" => meshfit::run(rest),
        "opcodes" => opcodes::run(rest),
        other => Err(format!("unknown command `{other}`")),
    };
    if let Err(e) = result {
        eprintln!("victory-census {cmd}: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(target_vendor = "apple"))]
fn main() {
    eprintln!("victory-census runs on macOS only (Metal Shader Converter and Metal)");
    std::process::exit(2);
}
