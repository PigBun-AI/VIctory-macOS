# Mac patches on `feat/live-translator`

Branch `mac/livetr` is `feat/live-translator` (the Windows line, base `150c16b4`) plus the patches
below. Every commit that is ours starts with `mac:`. The aim: new files and `cfg(...)`-gated
modules for the Mac, the fewest and shortest edits to files the Windows line owns, so that
`git merge win/feat-live-translator` stays cheap (`git log --oneline 150c16b4..mac/livetr` lists ours).

Facts as of **2026-11-27** (when written and checked): the Windows line is the user's line
(Marvisak ffe310eb2 → fxceless → avatarmanz 7900XTX → codec-d/cool_fox → the user's commits),
which has **no** Mac or Rosetta code; the Mac patches come from VIctory-macOS `origin/main`
7772921b (612 commits ahead of Marvisak/VIctory `main` 63bcca43, most of them generic upstream
PRs that are *not* ported: see `inventory.md` next to the integration notes).

Size: 23 new files, 21 existing files edited (105 lines added, 7 removed), everything else new.

**Not verified by a run.** This round only builds and tests: native `cargo build/test` on macOS,
and the cross build of `victory.exe` (`x86_64-pc-windows-gnullvm`, release). Nothing below was run
on Wine/Rosetta/D3DMetal; the Windows-only tests of the new modules compile for the Windows target
but have not been executed there (`tools/macos/test-wine.sh` runs them under Wine).

## Building

```sh
. tools/mac/env.sh        # reuses VIctory-macOS/target/toolchain (Rust 1.98.1, llvm-mingw); nothing is installed
tools/mac/check.sh        # native build, native tests, cross build of victory.exe; logs in target/mac-logs
```

`RUSTUP_TOOLCHAIN` is pinned to the version in `rust-toolchain.toml`: that file also lists the
Windows msvc target, which rustup would otherwise try to install. The gnullvm linker comes from the
environment (`tools/mac/env.sh`), as this line's `.cargo/config.toml` has no entry for it (the Mac
fork's has one, pointing at `tools/macos/gnullvm-cc.sh`, which needs Homebrew's llvm-mingw).

## Patches

### M1 — builds and tests natively on macOS (commit `31b87a11`)

*What.* The Windows line only compiles on Windows: new code names `cfg(windows)` modules and
`windows-sys` by path. Gates, in the Windows line's files: `victory-loader/lib.rs` (`mod crash`
gated, 1 line), `victory-gpu/resource.rs` (`view_format` gated, 1 line), `victory-gpu/lib.rs` (a
block at the end naming stand-ins), `victory-hle` (`lib.rs`/`runtime.rs` `image_watch`,
`modules/d3d12x.rs` XMem hooks, `modules/xgameui.rs` dialog, `modules/kernel.rs`
`SetThreadPriority`), `apps/victory/main.rs` (`translate` gated). Tests that read their own binary as
a PE file or need Windows paths are `ignore`d off Windows (`victory-pe/image.rs`,
`victory-loader/prepare.rs`, `victory-hle` `kernel.rs`/`storage.rs`).
New files: `victory-gpu/src/mac/nonwindows.rs` (stand-ins for `crate::host::read_into`,
`texture::committed`, `shader_cache::{counts,translation_of}`; this is **G1**) and
`apps/victory/src/mac/translate_stub.rs` (`translate`/`shaders` say they need Windows).

*Why the Windows line does not need it.* It is built and tested on Windows only.

*Remove when.* The Windows line gates these itself (check with `cargo build --workspace` on the
merged tree), or native Mac `cargo test` is no longer wanted. When a gate becomes redundant the
merge shows it as a conflict or a duplicate `#[cfg(windows)]`: drop ours.

### M2 — Wine, SDL2 and the launcher scripts (commit `52561683`)

*What.* `setup.sh`, `tools/macos/*` (CrossOver Wine build, deps, `victory.sh`, `test-wine.sh`,
`gnullvm-cc.sh`, `native-build-tools.sh`, `dxc-*.sh`) and the two Wine source patches
(`ntdll-skip-host-mappings`, `wineserver-view-tree`). All new files, copied byte for byte from
VIctory-macOS 7772921b except `victory.sh` (2 small edits: linker default, DXC helper only if
`victory-gpu/build.rs` reads `VICTORY_HELPER_DXC`, which this line's does not yet: see G3) and
`xtask/src/policy.rs` (allows the `.patch` extension, 2 lines).

*Why not needed on Windows.* Windows runs the exe directly.

*Remove when.* The Wine route is retired (the Metal backend runs natively on macOS) or CrossOver
Wine ships the two patches. Checked 2026-11-27: CrossOver Wine 26.3 (wine-11.0), SDL2 2.32.10,
D3DMetal 4.0 beta 3 (GPTK 4.0 beta 3), llvm-mingw 20261117, Rust 1.98.1, Homebrew 7.0.16,
macOS 27.2.1 on an M5 Max, Rosetta 2 installed (macOS 28 drops Intel apps: the Wine route ends with it).

*Fix, 2026-11-27 (first Wine test run).* `tests/fixtures/hle-probe/build.rs`, which `tools/macos/test-wine.sh`
builds first, passed MSVC's `/DELAYLOAD:` to clang (`no such file or directory`) on `x86_64-pc-windows-gnullvm`.
The file is now the Mac fork's (`-Wl,--delayload=` for gnullvm, a dlltool import library for plain gnu, MSVC
unchanged). Remove when the Windows line's `build.rs` picks the right form for gnullvm itself.

### M3 — `tools/mac/env.sh`, `tools/mac/check.sh`, this document

*Fix, 2026-11-27 (first run on Wine).* llvm-mingw's clang linked `libunwind.dll`, which Wine does not have
(`Library libunwind.dll not found`, c0000135, exit 53 at start). `env.sh` now names `tools/mac/cc.sh`, a
wrapper that swaps `-lunwind` for llvm-mingw's static `libunwind.a` (what `tools/macos/gnullvm-cc.sh` does
with `libgcc_eh` on Homebrew). An exe built before this fix must be relinked.

New files only. `env.sh` reuses the toolchain of a sibling `VIctory-macOS/target/toolchain` (or
`VICTORY_MAC_TOOLCHAIN`). Remove when the Mac fork's `setup.sh` is the one way to a toolchain.

### G1 — see M1 (the stand-ins of victory-gpu).

### G2 — fault tracking under Wine on Rosetta: on by default, `VICTORY_FAULT_TRACKING=0` turns it off

*What.* `victory-gpu/src/mac/rosetta.rs`: `rosetta()` (ntdll `wine_get_host_version` says "Darwin"),
`fault_tracking()` (on by default everywhere, Rosetta included; `VICTORY_FAULT_TRACKING=0` forces it off, `=1` on),
`stores_need_patching()` (tracking on and Rosetta), `wait_for_overlapped_reads()`. Hooks (unchanged since the
first version): `watch.rs` `writes()` returns `None` when tracking is off (every caller then copies the title
memory whole), `xmem.rs` leaves out `MEM_WRITE_WATCH` on the pools, `files.rs` `ReadFile` waits for an
overlapped read when tracking runs under Rosetta; `victory-hle` re-exports the module for the app. Only the
default changed (it was "off under Rosetta"): the file is Mac layer, no upstream file is touched.

*Why it can be on now (G6, L1).* It was off because Rosetta cannot resume a store that faults partway
("unexpectedly need to EmulateForward on a synchronous exception"; VIctory-macOS #1, #14, #50, #144) and the
first run found our own `vpmaskmovd` in `safecopy.rs` doing just that. G6 (`b7943221`) put plain `mov`s in
its place, and with tracking on the loader patches the title's masked stores at load (L1), the 8 extra sites
of #144 included (log: `masked stores  1242 patched to plain stores`, with no `VICTORY_PATCH_MASKED` set).
Runs with tracking on after G6 (3 before the change, the stability series after it, see the report
`2026-11-27-tracking-report.md`) show no EmulateForward. Off, a world frame copies ~15 GB of title memory
(~4 s a frame); on, the copies are the changed pages only.

*Why not needed on Windows.* Windows resumes a store that faults partway. On a PC `rosetta()` is false and
every hook does what it did before.

*Test.* `mac::rosetta::tests::the_variable_decides_and_tests_track`: the variable decides, the default is on,
`stores_need_patching` and `wait_for_overlapped_reads` follow from tracking and `rosetta()`, which holds on a
PC, under Wine on a Mac and natively.

*Remove when.* Never needed on a PC; here: the Metal backend replaces the D3D12 path, or a Rosetta release
resumes all such stores (then L1, L2, L4, G6 go too). *Take the default back to off if* a Rosetta or tracking
fault (EmulateForward, a hang in `titlewrite`/`shadow`/`inplace`/`safecopy`) shows up in the stability runs
(`VICTORY_FAULT_TRACKING=0` is the switch meanwhile).

### G4 — helper shaders compiled by D3DMetal's DXC (`victory-gpu/src/mac/hlsl.rs`)

*What.* Wine's `d3dcompiler_47` (vkd3d-shader) cannot compile the runtime's helper shaders: no methods on
`RWByteAddressBuffer` (`E5005 Method 'InterlockedOr'/'Load'/... is not defined`), no stores to indexed
`groupshared` arrays, no `[RootSignature]` attribute (so no root signature in the blob), no
`[allow_uav_condition]`. The indirect argument clamp is one of them: **without it every indirect
execution is dropped and the title waits for the result for good** (run 2026-11-27: the world load hung
at frame ~1150, every time). New `mac/hlsl.rs` has `d3d_compile`, same signature as `D3DCompile`; the
six files that compile such shaders (`indirect.rs`, `inplace.rs`, `compose.rs`, `frame_thumb.rs`,
`host.rs` x2, `replay/play/merge.rs`) import it in place of `D3DCompile` (one line each, marked `mac G4`).
Under Wine on a Mac it sends the source to the DXC that D3DMetal ships (`libdxcompiler.dylib`, x86_64,
via `tools/macos/dxc-d3dmetal.sh`'s front end, started from Wine as a Unix program through `/bin/sh`
and waited for by a `.done` file: Wine's handle for a Unix process never signals), raising `cs_5_x` to
`cs_6_0`; the DXIL is kept by hash in `VICTORY_DXC_DIR`, so each shader is compiled once per machine.
Falls back to `D3DCompile` where the helper is not configured or fails (and logs why).
`tools/mac/run.sh` builds the front end and sets `VICTORY_DXC_HELPER`, `VICTORY_DXC_DIR`, `DXC_LIB`.
`VICTORY_MAC_HLSL=0` turns it off. Same idea as VIctory-macOS #301 (precompile the clamp at build time),
but at run time and for all helper shaders, so the Windows line's `build.rs` stays as it is (G3 stays open).

*Fix (found by the Wine test run).* The command line sent to `/bin/sh` began with `exec`, so the shell was replaced by
the helper and never ran the `: >…done` that `run_unix` waits for: every helper shader waited the full 120 s on its
first compile (then found the result file and went on). Without `exec` a cold shader takes about 0.2 s.

*Why not needed on Windows.* FXC compiles all of them. *Remove when.* Wine ships a `d3dcompiler_47` or
`dxcompiler.dll` that takes them, or the Metal backend replaces the D3D12 path.

### G5 — which adapter the host device is on (`mac/adapter.rs`)

One log line per DXGI adapter at device creation (`host.rs`, one line). Result on this Mac: a single adapter,
`"AMD Compatibility Mode"`, vendor 0x1002, device 0x66af, 110,100 MB, flags 0 (a hardware adapter): that is
D3DMetal's default presentation of the Apple GPU (it has `D3DM_VENDOR_ID`, `D3DM_DEVICE_ID`,
`D3DM_DEVICE_DESCRIPTION` to change it); wave lanes 32/32, so `victory_shader::emit::set_host_rdna` stays
off while `indirect::amd()` is true. Nothing is selected wrongly: there is no other adapter.

### M4 — `tools/mac/run.sh`: the one way to start the title on a Mac

One script for both backends (D3DMetal and our Metal backend) and both output profiles; usage, every option and
variable and the defaults are in `docs/mac/RUN.md`. What matters for the patches:

- **Backend** (`--backend d3dmetal|metal`) is the only difference between two runs, besides the data directories (below): the prefix (`target/wineprefix-livetr-d3dmetal`
  or `-metal`, never shared), the dlls in its `system32` (the Metal ones are installed with the originals kept as `.metal-orig`,
  and put back when the D3DMetal backend is chosen), `WINEDLLPATH`, `WINEDLLOVERRIDES`, `VICTORY_METAL_LOG`,
  `VICTORY_METAL_NOTIMPL_LOG`. `run.sh --print-env --backend d3dmetal` against `--backend metal` shows exactly that.
- **Profile** (`--profile fidelity|perf`, default **perf** for now, see "Profiles and when to switch" below).
- Everything else is set the same way for both: the helper DXC (G4: `VICTORY_DXC_HELPER`, `VICTORY_DXC_DIR`, `DXC_LIB`), the input
  (I1: `VICTORY_KEYS` 8-3600/3:Enter, `VICTORY_INPUT_REPLAY` the neutral input), `VICTORY_FAULT_TRACKING` (1), the masked-store
  patch (follows tracking, not set), `VICTORY_PRESENT_MAX` (P1), `VICTORY_POOL_CHECK` (L3), `VICTORY_LOG_RENDER_SIZE` (P2),
  `WINEPREFIX` (explicit; only that prefix's wineserver is ever stopped), the data, cache and log directories, recording and
  frame capture options, `--sample` (memory/GPU lines in `mem.csv`).
- **Data directories**: `victory-data-livetr-<profile>-<backend>`, cloned from the read-only seed `victory-data-livetr`; `--fresh-data` clones
  again. Both backends start from the same state and can run in parallel. These directories and the prefix are the allowed differences
  of `--print-env` between backends, besides `WINEDLLPATH`/`WINEDLLOVERRIDES`/`VICTORY_METAL_LOG`/`VICTORY_METAL_NOTIMPL_LOG`.
- `--print-env` prints what would be passed (sorted, `~` for the home directory) and the command, and does nothing else.
- Each run keeps `run-env.txt`, `start.txt`, `summary.txt` next to its logs.
- Refuses: the Metal backend on a D3DMetal prefix (`wineprefix-play`, `wineprefix-livetr`, `wineprefix-livetr-d3dmetal`); a start while
  another `victory.exe` runs (two games at once ran a 32 GB Mac out of GPU memory; `--allow-concurrent`).

*Not verified by a run:* the script was exercised with `--print-env` and with a stand-in Wine (data copy, `settings.xml` edit,
dll install and restore, watchdog, sampler); no title was started.

### M5 — `tools/mac/wine-tests.sh` (tests under Wine, with the DXC front end)

`tools/mac/wine-tests.sh [cargo test args]` runs `tools/macos/test-wine.sh` with its own prefix
(`target/wineprefix-tests`; it refuses `wineprefix-play`, and stops only the wineserver of the prefix it used,
since `tools/macos/victory.sh` without `WINEPREFIX` uses `wineprefix-play` and runs `wineserver -k`, which
kills a game running there) and with `VICTORY_DXC_HELPER`, `DXC_LIB`, `VICTORY_DXC_DIR` set (front end built
once by `tools/macos/dxc-d3dmetal.sh` into `target/dxc-d3dmetal`), so that G4 compiles the runtime's helper
shaders in the tests too. Without it 8 tests of `victory-gpu` fail on `d3dcompiler_47` (indirect clamp,
worker shaders, `ambient::the_passes_build`, the depth-plane drawing). This is the whole port of the Mac
fork's `VICTORY_HELPER_DXC` for tests: that variable is read by the fork's `victory-gpu/build.rs` to
precompile the clamp at build time; this line compiles at run time (G4), so `build.rs` stays untouched (G3
is closed by G4 + this script). Remove with G4.

### Test results under Wine on Rosetta (2026-11-27, CrossOver Wine 26.3, D3DMetal 4.0 beta 3, not fixed)

Run with `tools/mac/wine-tests.sh -p victory-gpu` / `-p victory-shader`. The adapter is D3DMetal's single
"AMD Compatibility Mode" (G5), there is no software adapter (`EnumWarpAdapter` gives the same card, flags 0,
which `warp_adapter()` refuses), so tests that need a WARP reference print `no SM 6.6 warp device` and
return: they pass without comparing anything.

`victory-gpu` (9 fail of 384; 35 ignored):
- Wine: `watch::{holes_are_known_until_the_runtime_tells, no_mapping_is_lost_between_askers,
  what_is_left_alone_is_no_known_hole}`: Wine's `NtQueryVirtualMemory` has no class 7
  (`MemoryRegionInformationEx`), so `watch::placeholder()` is never true (only matters with
  `VICTORY_FAULT_TRACKING=1`).
- Rosetta: `titlewrite::decode::stores_do_what_the_processor_does` compares the flags of `lock cmpxchg [r11], dl`
  with the decoder's (0x881 against Rosetta's 0x80): Rosetta does not compute them alike.
- D3DMetal: `root::malformed_blobs_are_rejected_by_the_host` and
  `pipeline::library::a_rejected_blob_gives_an_empty_library` (it accepts malformed blobs),
  `descriptor::host::{views_are_made_beside_the_heap_and_copied_in, a_raw_view_is_moved_only_where_its_descriptor_says_where_it_is}`
  (a CPU descriptor handle is not readable memory there).
- Not explained: `indirect::host_list_budget_starts_empty_at_each_submission` (the small record runs after the
  budget was spent: 1 hit, want 0).

`victory-shader` (255 pass, 36 fail, 6 ignored of 297): the same 36 with or without the DXC front end, so
none is the missing helper. They are results of the shader on the hardware device that differ from what the
test wants (zeros or wrong values), with no WARP reference to compare: f16 and d16 forms (7), wave, permlane
and quad masks (7), unorm and typed buffer formats (4), f64/i64 (3), float edge cases of v_fract, v_med3,
v_min3, v_sin/v_cos, v_cvt_off, ds_min/max (7), pixel shader tests that draw (barycentric, edge flags, quad
neighbours: 3), one accepting an unsigned DXIL (`stub_pipeline_is_created`; there is no `dxil.dll`), and
raw twins / counter texture / waterfall / page cache (4). D3DMetal's DXIL-to-Metal compiler, not the Mac layer.
`whole_wave_f64_sum_and_cvt_f32_f64` is the one that ends the Metal backend's MSC (skipped there).

The victory-shader count (297) is not a skip: the Windows line's crate is another code base from the
Mac fork's (458 tests): per-file counts differ everywhere (`emit/lower/tests.rs` 93 and `halves_tests.rs` 45 are
the fork's, `bvh.rs`, `bitcode/opt.rs`, `wave64_tests.rs` are the line's) and no test is gated on `VICTORY_HELPER_DXC`.

### Findings of the first run under Rosetta (2026-11-27, not fixed)

- `VICTORY_FAULT_TRACKING=1`: the process stops with `rosetta error: unexpectedly need to EmulateForward on a
  synchronous exception` at `victory_safe_merge_at` of **`safecopy.rs`** (our own `vpmaskmovd` store into a
  title page the tracker guards, in `land.rs`' merge). The loader's masked-store patch covers the title's code, not
  this asm; `victory_safe_put` has the same store. Default (tracking off under Rosetta) does not guard pages
  and does not reach it. Fixed by G6 (per-dword plain stores).
- `titlewrite.rs`, `shadow.rs`, `inplace.rs`: no fault or hang seen with tracking off (default).

### G6 — safe copies without the AVX masked store (`victory-gpu/src/mac/safecopy.rs`)

*What.* `safecopy.rs` (the Windows line's) stores what the card changed with `vpmaskmovd` in two assembler
routines, `victory_safe_put` (used by `shadow::put_changed`) and `victory_safe_merge` (used by `land`), and
counts on a fault on a page that is missing or guarded being caught by `watch::on_write` and sent to the
failing exit. Rosetta 2 cannot take a fault of an AVX masked store (`rosetta error: unexpectedly need to
EmulateForward on a synchronous exception`, the process stops): it killed `safecopy::tests::a_merge_into_missing_memory_fails`
and `host::refresh_tests::refreshed_ranges_hold_title_memory` (the second writes through a read-only guard of
`watch`), whatever `VICTORY_FAULT_TRACKING` says. `loader/mac/masked.rs` (L1) rewrites only the *title's*
masked stores, not ours. Not a compiler artefact (the build has no AVX by default; the AVX2 functions only
load) and not the tests' own: it is production code, reached in the game by `land::merge` (every landing of
shader writes into title memory, `VICTORY_LAND` default on) and `shadow::put_changed` (every put). Under
Rosetta with tracking off nothing is guarded, so the fault only comes when the title unmaps or protects
memory the card writes into; with `VICTORY_FAULT_TRACKING=1` it comes at every first write to a guarded block.

*How.* `mac/safecopy.rs`: `victory_mac_put` and `victory_mac_merge`, the same routines with one plain 32-bit
`mov` per changed dword (loads of title memory stay 256-bit: a load that faults changes nothing), the same
arguments, results and failing exits. Hooks in `safecopy.rs`: the 3 call sites go through
`mac::safecopy::{put, merge}` (which pick the twin when `scalar()`: Rosetta, or `VICTORY_SAFE_SCALAR=1`;
`=0` never) and `failing_exit` asks `mac::safecopy::failing_exit` last. A test checks the twins against the
masked routines; the existing safecopy tests run through whichever is active.

*Not needed on Windows* (it resumes or takes the fault of a masked store). *Remove when* Rosetta handles
faulting masked stores (retest with `VICTORY_SAFE_SCALAR=0`), or with G2.

### G7 — host pipeline release diagnostic (`victory-gpu/src/mac/pso_grave.rs`, 4 lines in `pipeline/host.rs`)

*What.* `VICTORY_MAC_PSO_GRAVE=log` logs every host pipeline that is dropped ("host pipeline released", with
the frame and shaders); `=1` also keeps a reference to each, forever. Off (default): nothing. Written to settle
whether the D3DMetal crash in `SetRenderVertexBuffers` (below) is a pipeline released while a list still uses it.

*Result (2026-11-27).* It is not. Over runs that went into the world and crashed, no host pipeline was dropped
before the crash (0 "released" lines), and `draw.rs` already keeps each bound pipeline in its list
(`list.bind_pipeline`). Setting the pipeline again before every draw, and a single pipeline worker, change nothing.

*Known issue (not fixed): D3DMetal 4.0b3 crash in the first scene of the world.* About 75% of the runs that
latch `world frame 1` (I1) fault ~10 frames later at `D3DMetal+0xf545f`
(`D3D12GraphicsCommandListMPL::SetRenderVertexBuffers`, reads the vertex buffer count at pipeline object +0x28c
and walks it in 40-byte steps). It comes in a frame with hundreds of first binds of pipelines made a moment
before. Excluded: pipeline lifetime (above), pipeline library, input layout in the stream, depth bounds,
fault tracking (on or off), worker count. Open: D3DMetal's own handling of the pipelines made by
`CreatePipelineState` (stream), which every mesh pipeline and every depth-bounds pipeline use. *Remove when* D3DMetal
or the Metal backend replaces the D3D12 path.

*Title crash at 0x2215c35 (previously taken for an out-of-memory path): it is the title's own hang watchdog.*
A thread of the title sleeps 1 s at a time and writes `[0]` on purpose when the frame loop has not called its
heartbeat (`title+0x22149f0`) for 30 s. It fired because the title waited for a helper shader (G4) whose `.done`
marker never came (`exec` in the command line: fixed by `465a121a`, "each helper shader waited 120 s"). Cold
helper cache with the `exec` build: crash 37 s after the start, every time; the fixed build with a cold cache goes
on to the world. No allocation failed (xmem logs no refusal).

### G8 — where the title's draws go: `VICTORY_MAC_DROP_STATS` (`victory-gpu/src/mac/drop_stats.rs`, `tools/mac/drop-ab.sh`)

*What.* A runtime switch (not the `diagnostics` feature: that one is Windows-only code that needs a diag build, and
`note_drop` / `note_why` print nothing in a release exe), so a plain `check.sh` release exe has it. `VICTORY_MAC_DROP_STATS=1` logs, per
present, `drop-stats present=P world_frame=W recorded=D+X ran=R dropped=N notimpl[0x1c8=a 0x218=b] why[class=n ...] kind[kind=n ...]`
and every 5 s `drop-stats summary` and 20 `drop-stats top` lines (n, op, pipeline kind, creation method, reason, example
shader hashes), once for the whole run and once from world frame 1. `=2` adds `psoran[gseq:n ...]` (host pipelines that
ran this present) and one `drop-stats pso` line per host pipeline made (`seq`, `gseq` = order among graphics and
mesh ones, kind, shaders). `VICTORY_MAC_DROP_STATS_PSO=<path>` rewrites a table of them every 5 s. Off: one relaxed
load per hook, nothing logged.

*Reasons* (the upstream `make::missing()` and the counters lump these; only the Mac layer tells them apart,
`pipeline::mac_state` / `mac_view` read the `Pending` without waiting): `compiling` (requested, not finished),
`failed:shader_translation(stage hash)` or `failed:<host reason>` (`host pipeline refused: <HRESULT>`, `no vertex or mesh shader`,
`the host GPU has no mesh shaders...`), `never_requested:left_out` (the ray-tracing resolve/activate programs; upstream calls it "being made"),
`rt_builder_off`, `skip_list`, `kind_mismatch`, `bind_failed`, `bind_or_index_failed`, `null_pipeline`; ExecuteIndirect:
`ind:` + the first reason `execute_indirect` gave up for (`arguments outside title buffers`, `count outside title buffers`,
`no host signature`, `mesh draws are switched off...`, `not bounded (no clamp, or arguments not 4-byte aligned)`, `prepare failed`),
`ind:signature_unsupported(...)`, `ind:no_signature_layout`, or the pipeline's state. Pipeline kinds: `mesh`, `vs`, `vs+gs`, `tess`, `compute`; host side
`vs+rect_gs`. Creation method of a title pipeline: `graphics@0x50`, `compute@0x58`, `compute@0x1f0`, `stream@0x2b0`
(slots 0x1c8 and 0x218 never make one: `device::unknown_pipeline` counts their calls per present).

*Hooks in upstream files* (one line each, `crate::mac::drop_stats::...`): `draw.rs` (the drop and run sites,
`bound_pipeline()`), `list.rs` (`count_draw`/`count_dispatch`, `execute_indirect` result), `pipeline.rs`
(`creating`/`created` in the four create methods, `MacState`/`mac_view`), `pipeline/make.rs` (`mac_state`, the host result),
`pipeline/host.rs` (the refusal reason), `device.rs`, `queue.rs` (present). Upstream semantics are unchanged.

*D3DMetal no-op pipelines.* D3DMetal prints `marking PSO(n) as no-op` on stdout for a pipeline it could not compile
after our create call succeeded; the shared layer counts such a draw as drawn. `tools/mac/drop-ab.sh` stamps
stdout with the time of each line and `tools/mac/drop-stats-report.sh` ties `PSO(n)` to our `seq` or `gseq` only if the
pipeline it names was created 0..10 s before the line for at least 90% of them (the numbering is not documented);
if so it gives the draws that ran on no-op pipelines (`psoran`), else only the lines per world frame.

*Run.* `tools/mac/drop-ab.sh` (warm = `--make-book` run if needed + `victory warm` + run, cold = empty cache, same
input script and Enter keys as `run.sh` has them; both through `run.sh --backend d3dmetal --perf|--fidelity --data --cache --out`,
and `victory warm` through `run.sh --exec -- warm`, so the prefix and the environment are `run.sh`'s) and prints the share of each class over the first N world frames. *Remove when* the Metal backend replaces the D3DMetal path.

### L1 — masked 256-bit stores rewritten (commit `781672bc`, `victory-loader/src/mac/masked.rs`)

AVX masked stores (`vpmaskmovd/q`, `vmaskmovps/pd`, `vcvtps2ph` to memory) redone as plain stores
Rosetta can resume. Done whenever fault tracking runs under Rosetta, which is now the default (G2), so it
needs no variable: a start with `VICTORY_PATCH_MASKED` unset (`tools/macos/victory.sh`, a plain `wine victory.exe`,
and `tools/mac/run.sh`, which no longer sets it) logs `masked stores  1242 patched to plain stores`.
`VICTORY_PATCH_MASKED`: `none` (no patch), `all`, or hex RVAs (explicit, taken as given). `apps/victory/src/mac.rs`
adds the 8 plain 256-bit stores of #144 (RVAs in `BUILD_1_0_2_1_EXTRA`, build 1.0.2.1, hash `b99f876b…`) when the
variable is unset **or is plain `all`**: 1234 found by the scan + 8 = 1242. Before, an explicit `all` (which `run.sh`
exported) replaced the list, so those 8 were never patched on a `run.sh` start; and with tracking off by default
nothing was patched at all without the variable. With `VICTORY_FAULT_TRACKING=0` nothing is patched (nothing is guarded).
Not needed on Windows: it resumes them. Remove with G2.

### L2 — 256-bit stores split into two 128-bit stores (`mac/wide.rs`, `xtask/src/wide_stores.rs`)

`VICTORY_PATCH_WIDE=<list>` (exe) and `VICTORY_PATCH_WIDE_DIR` (title DLLs; hook in `windows.rs`);
the list comes from `cargo xtask wide-stores <cached image>` (a disassembler pass, new xtask
command). Rosetta aborts on a fault in the second page of a 32-byte store. Remove with G2.

### L3 — title output size 1280x720 by default on a Mac (`mac/resolution.rs`)

`VICTORY_OUTPUT`, `VICTORY_RENDER_SCALE`, `VICTORY_UPSCALER`, `VICTORY_POOL_CHECK`; compares each site byte for byte
and leaves it alone if the build differs (build 1.0.2.1 only). The default 1280x720 applies on **any Mac** (the loader asks Wine
for its host system, `wine_get_host_version` = "Darwin", `resolution::on_a_mac`; `apps/victory/src/mac.rs` does not look at
Rosetta): it was documented as "under Rosetta only", which the code never did. Not needed on Windows (the user picks the size in the
title). The patch changes the title's *output*; the internal size follows (output x 2/3, 1280x720 gives 853x480).

- The output patch writes the size into the settings block (+0x204/+0x208) and, for a size other than 3840x2160, also jumps over the
  back buffer pool's check (+0x247af25: the pool, 64 MB on Series X, is sized for 3840x2160 and the check wants under 2 MB of it
  left; at 1920x1080 49 MB are left and the title stopped there, run res-1). The two sites are written together or not at all.
- `VICTORY_POOL_CHECK=skip` (new) jumps over that check alone, with the output left to the title. It exists for the fidelity profile:
  with `VICTORY_OUTPUT=0` and a settings size of 2560x1440 the check is expected to stop the title at its `int3` the same way
  (roughly 34 MB of the pool left if only the two 1440p back buffers use it, the same arithmetic; **not run**). It changes no size and no scale. Off unless set.
- **Rule for the fidelity profile (decided):** the pool check is *not* skipped by default. If a real fidelity run (F0) stops at
  +0x247af25, `--pool-check skip` is fixed into the fidelity profile in `run.sh`, the log gets a `non-fidelity: pool check skipped`
  line (a loader change then), and the node is registered in the matrix as a **stubbed** node: its numbers say the title ran with that
  one check jumped over, not that it ran on its own. Until a run stops there, nothing changes.
- When the output is patched the log says `non-fidelity: output patched to WxH` (stdout, once), besides `resolution: ... Done` per site.
- Which settings decide the output: see "Which `settings.xml` keys decide the output" below.

Remove when the Mac GPU path is fast enough for the title's own size.

### L4 — `clwb` loops become NOPs under Rosetta (`mac/clwb.rs`)

`clwb` faults under Rosetta (one exception, about 300 µs through Wine, per loop; the world load
crawled, #135). The Windows line already emulates `clwb` loops in `crash.rs` (one exception per
loop), fine on a PC but too slow through Wine, so the 8 loops of the supported build are patched
out at load (nothing is patched unless exactly 8 are found). The loop scan is a copy of
`crash.rs`'s `clwb_loop` (test included) so `crash.rs` is untouched; if the Windows line changes
the pattern, change the copy. `VICTORY_NOP_CLWB=0/1`. Remove with G2, or if Rosetta handles `clwb`.

### H1 — `SuspendThread` only for the title's own threads (commit `5050245c`)

New `victory-hle/src/modules/mac.rs`, registered in `modules/mod.rs` **only when `rosetta()`**,
plus `victory_loader::mac::in_title_image` (`threads.rs`, 8 lines). Under Wine on Rosetta a thread
of ours suspended while Wine held its virtual-memory lock left the lock held for good (#50).
Remove when CrossOver Wine fixes the lock (retest after a Wine bump from 26.3) or with G2.

### T1 — `victory-shader-dylib/tests/translate.rs`: corpus folders from the environment

The test had two build-machine paths of the Windows team written in (`...\build\livetr-corpus`,
`...\build\rt1-work`), which do not belong in a public history. Changed in place (about 6 lines,
Windows file, only the new `mac:` commit touches it; the Windows line's history is as it was):
`VICTORY_LIVE_CORPUS` now holds one or several folders (separated like `PATH`: `;` on Windows), and
when it is unset the container test returns with a "skipped" message instead of asserting. To get
the old behaviour on the Windows machine, set `VICTORY_LIVE_CORPUS` to the two folders. Remove this
patch if the Windows line makes the same change.

### M5 — `tools/mac/push-check.sh`, `docs/mac/PUSH.md`

The check before a push to the public `fork`: policy, credential scan, privacy scan (internal
addresses, home-directory and drive paths, `*.local` e-mail addresses, the author of `mac:`
commits, a private word list kept outside the repository), binaries and game files. Also
`.gitignore` entries for `tools/mac/privacy.local.txt`. Usage in `docs/mac/PUSH.md`. Pure Mac
tooling, nothing to recycle; delete it if pushes to the fork stop.

### I1 — input record and replay by world frame, `VICTORY_KEYS` repeats

*What.* Ported from VIctory-macOS (`gameinput_script.rs`, #423, and the repeat form of #392); all of it is
off unless a variable is set, and with none set the title reads the host exactly as before.

- `VICTORY_INPUT_RECORD=<file.jsonl>`: writes each change of a pad or of the held keys, tagged with the
  world frame. Refuses a path inside a source tree (`.victory-root`).
- `VICTORY_INPUT_REPLAY=<file.jsonl>`: feeds the title from a recorded file or from range lines such as
  `{"from":0,"to":120,"ly":1.0,"rt":0.8,"buttons":4,"keys":[87,"Enter"]}`; `tools/mac/input/world-neutral.txt`
  (`{"from":0,"to":100000000}`) is the neutral input. While the world frame is 0 (menus, loading) the pad
  stays neutral and the keyboard, `VICTORY_KEYS` included, passes through; from world frame 1 the replay is
  exclusive. Same lines work on D3DMetal and on the Metal backend: the clock lives above the host.
- `VICTORY_KEYS=<from>-<to>/<every>[+<hold>]:<key>` (e.g. `8-900/3:Enter`): presses the key again every
  `<every>` seconds, each press `<hold>` long (0.2 s by default); times may use scientific notation. The old
  forms parse to the same schedule (a test checks it against a copy of the old parser).
- `VICTORY_WORLD_DRAWS=<n>`: the draws that make a presented frame the world's first (default 500).

*World frame.* `victory_gpu::mac::world::world_frame()`: 0 until the first present with at least 500 draws
recorded by the title since the previous present, then 1, 2, ... one per present. Counted from the title's
own draw recordings, not from draws run on the host (which lag a present with async submits and differ
between hosts), so both backends latch on the same frame. With a recorder or replay set, the log says at
which present it latched (`world frame 1`), the number `VICTORY_CAPTURE_FRAMES` uses.

*Files.* New: `victory-gpu/src/mac/world.rs`, `victory-hle/src/modules/gameinput_mac/{mod,keys,json}.rs`,
`tools/mac/input/world-neutral.txt`. Hooks in the Windows line's files: `victory-gpu/src/queue.rs` (1 line, at
the present), `list.rs` (1 line, `count_draw`), `mac/mod.rs` (1 line), `victory-hle/src/modules/gameinput.rs`
(the `mod` declaration, 1 line in `Hub::start`, and the Windows host's `scripted()` now calls
`gameinput_mac::keys::scripted()`), `victory-hle/Cargo.toml` (`victory-core`, for the repo-path check),
`Cargo.lock`.

*Differences from VIctory-macOS.* The world frame counts the title's recorded draws (above); no benchmark
JSON (`VICTORY_BENCH_*`), so no `VICTORY_BENCH_EXIT_FRAMES`; `VICTORY_KEYS_FILE` is not ported; the stand-in pad
is made from the keys by this line's hub, so there is no `Source::scripted_keys`; replay `keys` also take key
names (`"Enter"`).

*Why the Windows line does not need it.* It has no deterministic input; its runs are driven by a person or by
`VICTORY_KEYS` in wall-clock seconds. *Remove when.* The Windows line has a replay of its own (then drop
`gameinput_mac/` and the hooks, and keep `world.rs` only if a consumer is left), or `VICTORY_KEYS` takes the
repeat form upstream (then `keys.rs` and the `scripted()` call go).

### P1 — `VICTORY_PRESENT_MAX`: a smaller host window without a smaller title output (`victory-gpu/src/mac/present_cap.rs`)

*What.* `VICTORY_PRESENT_MAX=WIDTHxHEIGHT` caps the host window and its swap chain, keeping the aspect ratio and never
enlarging (`fit_output`, with the old base's tests). The title's output size, render targets and upscaler are not
touched: the picture it presents is drawn scaled (bilinear) into the smaller back buffer by `compose.rs`
(`Composer::record_scaled`: the same pass as the two-plane blend with one plane and a pixel shader `ps_scale`, its
pipeline made on first use). With two planes (Series S) the blend already scaled both to the back buffer, so only the
back buffer's size changes. Where the picture cannot be sampled (`compose::view` says no) or the pass fails, it is
shown at its own size and the log says why. Logs: `VICTORY_PRESENT_MAX: ...` once at start, `...the picture is drawn
scaled into the host's smaller swap chain` per size, `presenting to a window` with the capped size.

*Origin.* VIctory-macOS `d093abb1` (compute pass in `composite.rs`, `host.rs`); that line's presenter differs, so
the behaviour and the size rule are ported, not the shader. Until now this line had the variable in `macos.md` only.

*Files.* New `mac/present_cap.rs` (+1 line in `mac/mod.rs`); `compose.rs` (a second pixel shader and pipeline, the
draw split from `record`), `host.rs` (`present_planes` calls `present_capped`; the two-plane block takes an optional
overlay), all marked `mac P1` or commented.

*Test.* `mac::present_cap::tests` (the size rule and the value's syntax; native). The GPU draw is **not run**: it
needs a run under Wine, where the check is the log lines above and a window of the capped size.

*Why not needed on Windows.* The window is fitted to the work area by `lifecycle.rs` and the swap chain stretched.
*Remove when.* The Windows line has a presentation cap of its own.

### P2 — `VICTORY_LOG_RENDER_SIZE`: the title's internal render size in the log (`victory-gpu/src/mac/render_size.rs`)

*What.* Off unless `VICTORY_LOG_RENDER_SIZE=1` (with it off each hook is one atomic load; with it on, a lock per draw, so not
for timing runs). Prints lines starting `render size:` on stdout:

- `render size: target colour|depth WxH (samples N, K sizes so far)`: each 16:9 colour or depth render target size the title makes
  (the first 300 sizes);
- `render size: world frame N: internal WxH (scale S of the output WxH; D draws of T below the output size, X at the output size)
  confirmed=yes|no top=[WxH:draws ...]` at world frames 1, 2, 5, 10, 30, 100, 300, 1000, then every 3000th (world frame: patch I1);
- `render size: changed: AxB -> CxD at world frame N` whenever the frame's internal size differs from the previous frame's
  (dynamic resolution shows up here; at most 200 lines).

*How the internal size is decided (the reliable rule).* The title's render size is its output times its upscaler's scale
(`+0x7e9440`, clamp 0.2..1.0, initial 2/3), computed in its own code and not stored anywhere readable at load, so it is read off
what the title does:

1. Every draw is counted under the viewport it was recorded with (the first viewport of the last `SetViewports` on the recording
   thread). At each present the counts of the frame are taken.
2. Candidates are viewport sizes that have **the output's aspect ratio** (within 1 %) and are **smaller than the output** by area.
   Square or odd-shaped targets (shadow maps, atlases) and the output-sized passes (the upscaler's result, post effects, the
   interface) drop out. The **internal size is the candidate with the most draws** (a tie goes to the larger). Half-size passes
   (ambient occlusion, bloom) are candidates too but have far fewer draws than the geometry passes at the internal size.
3. With no candidate drawn, the internal size is the output's (render scale 1.0).
4. `confirmed=yes` when the title also made both a colour and a depth render target of exactly that size. It is corroboration, not
   the verdict (a target can be created with padding, or before the log started).
5. The output is the size of the picture the host presents, so the ratio is internal / presented.

*Checks to make with it.* (a) Calibration: with the perf profile (output 1280x720) the verdict must be 853x480 (2/3), the figure
known from the earlier runs. (b) Fidelity profile at 2560x1440 the rule gives 1707x960 if the scale stays 2/3. (c) A scale other
than 2/3, or a `changed:` line in a steady scene, is the sign of dynamic resolution, which nobody has shown yet. Unit tests cover the
rule (`mac::render_size::tests`, native); the hooks are not run: there is no run of the title with it yet.

*Hooks in the Windows line's files* (one line each, marked `mac P2`): `list.rs` (`viewports`), `resource.rs`
(`hand_out_resource`), `host.rs` (the presented size). `mac/world.rs` calls it from `draw` and `on_present`.

### Which `settings.xml` keys decide the output (checked 2026-11-27, build 1.0.2.1)

The file is `<data dir>/<title id>/storage/settings/settings.xml` (`rage__fwSystemSettingsCollection`, CRLF). Only **`video/resolutionFullscreen/width`
and `/height`** decide the output size. Evidence:

1. The settings block the title applies at start (`+0x22434a0`) hands `[block+0x204]` and `[block+0x208]` to the one writer of the screen
   size (`+0x2478130`); everything else reads that screen size afterwards (device parameters, render size, back buffers). The L3 patch
   writes the size it wants into exactly those two fields (`resolution.rs`, `OUTPUT`).
2. Round trip: the saved `settings.xml` files of our data directories (identical where compared) read `<resolutionFullscreen><width 1280><height 720>` after runs in which
   the patch wrote 1280x720 into `+0x204/+0x208` (an unpatched Series X writes 3840x2160, PR #393): the title saves what is in those fields,
   so those two fields are the two values of that element.
3. The other keys of the file do not carry it: `video/uiResolution` is `0x0` in the same files (not touched by the patch), `refreshRate`
   (60/1), `adapterIndex`, `outputIndex`, `frameRateLimit`, `vSync`, `tripleBuffered` (in the same routine `+0x20c` is a flag byte and `+0x210`
   a value capped at 1000 that becomes a frame time: not the size) and `videoCardDescription` ("Xbox Series X", the console model).
4. The title's own display mode list is 1280x720, 1664x936, 1920x1080, 2560x1440, 3840x2160 at 60 Hz (`+0x615c0c0`), so 2560x1440 is a
   mode it has.

Not proven: that `uiResolution` stays `0x0` ("same as the output") at another output size, and what the title does with a size outside its
mode list (the fidelity profile only writes modes from the list). The first fidelity run shows both in the log.

### Profiles and when to switch (not switched yet)

`tools/mac/run.sh --profile fidelity|perf` (docs/mac/RUN.md):

| | **perf** (the default for now) | **fidelity** |
|---|---|---|
| console | Series X (or `--console series-s`) | Series X only |
| output | patched to 1280x720 (`VICTORY_OUTPUT=1280x720`, L3), log `non-fidelity: output patched to 1280x720` | the title's own, from `settings.xml` set to 2560x1440 in the profile's own data directory copy; `VICTORY_OUTPUT=0`, no resolution patch |
| internal size / dynamic resolution / TAA upscale | the title's, at 2/3 of 1280x720 = 853x480 | the title's own logic; expected 2/3 of 1440p = 1707x960 (`VICTORY_LOG_RENDER_SIZE`) |
| render scale, upscaler overrides | allowed (`VICTORY_RENDER_SCALE`, `VICTORY_UPSCALER`) | refused |
| pool check (+0x247af25) | skipped with the output patch | kept. If F0 stops there, `--pool-check skip` becomes a fixed part of the profile (rule below) |
| data / cache directory | `victory-data-livetr-perf-<backend>` | `victory-data-livetr-fidelity-<backend>`; each cloned from the read-only seed `victory-data-livetr` on first use (`--fresh-data` clones again); the game directory is never written |

The default stays **perf** until the 3D-missing question is settled and the owner says so; then the switch is one line,
`profile="${VICTORY_MAC_PROFILE:-perf}"` -> `fidelity` in `run.sh`, plus (separately, as a loader change) making `VICTORY_OUTPUT=0` the
code's default in `resolution.rs` `default_output_on`, so that a start without `run.sh` agrees. Perf stays as the explicit profile for
debugging and speed work, marked `non-fidelity` in the log, and no fidelity result may cite a perf run.

## Not ported this round (decided with the Windows line as the reference)

Listed in `inventory.md` with source commits. In short: D3DMetal/Metal-specific GPU work
(`alias.rs` `mach_vm_remap` zero-copy buffers, indirect-clamp/DXC precompile, castable lists,
R32G32B32 views, 64-bit atomics, mesh limits, queue recovery, shader MD5 hash, wide indirect
dispatch) because `host.rs`/`draw.rs` diverge by tens of thousands of lines; **G3**: the DXC
helper (`VICTORY_HELPER_DXC`) is skipped in `victory.sh` until `victory-gpu/build.rs` has it;
low-address guards (#398) and 2 TiB views, DirectStorage per-worker handles (#400), publish rename
tolerance (this line's `publish.rs` already stages per writer), pipeline cache calls under Wine;
the generic upstream PRs in the Mac history (not Mac specific). Each is a candidate for a later
`mac:` commit once a run on the Mac shows the need.
