//! End-to-end test of `d3d12.dll` / `dxgi.dll` under Wine, with real GPU work.
//!
//! The same exe runs against Wine's builtin D3DMetal (`WINEDLLOVERRIDES`
//! `d3d12,d3d12core,dxgi=b`) and against our dlls over the Metal back
//! (`d3d12,dxgi=n,b` and `WINEDLLPATH` pointing at the staged unixlib).
//! Output, one line each:
//!
//! * `E2E PASS name` / `E2E FAIL name: why` / `E2E SKIP name: why`
//! * `PERF name p50_us=… p99_us=… min_us=… n=…` (round-trip timings)
//! * `INFO …` (adapter, throughput, things to look at by eye)
//! * `E2E SUMMARY pass=… fail=… skip=…`
//!
//! Exit status 0 when nothing failed. Only copies, barriers, fences and
//! descriptor/root-signature creation are used: compute pipelines need the
//! shader converter. `tools/metal/test-front-smoke.sh --real` and
//! `tools/metal/test-e2e.sh` run it.

#[cfg(not(windows))]
fn main() {
    eprintln!("front-e2e runs on Windows targets (under Wine) only");
}

#[cfg(windows)]
fn main() {
    std::process::exit(e2e::run());
}

#[cfg(windows)]
mod e2e;
