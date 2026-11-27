//! ABI smoke test for `d3d12.dll` / `dxgi.dll` under Wine.
//!
//! The same exe runs against Wine's builtin D3DMetal (`WINEDLLOVERRIDES`
//! `=b`) and against our dlls (`=n,b`). Every line is printed as
//! `GOLD tag: text` (must be equal on both) or `INFO tag: text` (printed to
//! be compared by eye: adapter names, addresses and the like).
//! `tools/metal/test-front-smoke.sh` runs both and diffs the GOLD lines.
//!
//! It calls the dlls the way victory-gpu and victory-shader do: through the
//! `windows` crate.

#[cfg(not(windows))]
fn main() {
    eprintln!("front-smoke runs on Windows targets (under Wine) only");
}

#[cfg(windows)]
fn main() {
    smoke::run();
}

#[cfg(windows)]
mod smoke;
