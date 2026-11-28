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

*Fix, 2026-11-27 (exit status).* `check.sh` piped each cargo command through `tee` and `tail`, so the script's status was
that of the last pipe stage: a failed cross build (error in the log, stale `victory.exe` listed) still returned 0, and the
native steps' `grep ... || true` hid a failed test run the same way. Each step now writes to its own log, is summarised
afterwards, and is judged by cargo's own exit status; all steps run, each failure prints `!! step failed: <step> (exit N)`,
the last line names them (`check FAILED: cross build, native tests`) and the status is 1; a stale exe is not listed after a failed cross
build. Still plain `sh` (`sync-windows.sh` runs it as `sh tools/mac/check.sh`; `sync-windows.sh` already turns a non-zero status
with three green logs into a failed step).

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
of #144 included (log: `masked stores  1241 patched to plain stores`, with no `VICTORY_PATCH_MASKED` set).
Evidence (2026-11-27, 15 default-config runs after the change, 10 of them the series with neutral input and
Enter every 3 s, release exe of commit `4ab34ad8`): no EmulateForward, no title-watchdog crash, no warning or error
from `titlewrite`, `shadow`, `inplace`, `safecopy` or `watch`; one run lasted 7 minutes into the streaming world
(~870 frames). The runs that stopped (12 of 15) all stopped on the known D3DMetal `SetRenderVertexBuffers` crash (G7), and
at the same rate as with tracking off (6 of 7), so tracking does not change it. Off, a world frame copies ~9-13 GB of
title memory (median 2.9 s a frame over the first 100 world frames); on, ~80 MB (median 0.31 s, p95 0.54 s).
One title `int3` at `image+0x31f50` (frame 64, intro) was seen once in a diag run; not reproduced (2 more diag runs).

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

One script for both backends (our Metal backend, the default and only main path; D3DMetal, reference-only) and both output profiles; usage, every option and
variable and the defaults are in `docs/mac/RUN.md`. What matters for the patches:

- **Default backend: `metal`** (user decision 2026-11-28): our Metal backend is the one main path. D3DMetal stays installed and selectable
  with `--backend d3dmetal`, **reference-only**: pixel reference for openings and menus (what it draws correctly) and the final
  performance comparison; `run.sh` prints `d3dmetal: reference-only backend (openings/menus pixel reference, perf comparison)` on
  stderr and into `--print-env` / `run-env.txt`.
- **Stage resolution** (metal): `$VICTORY_METAL_OUT`, else `$ROOT/metal/target/wine-stage` (if it has `front/d3d12.dll`), else
  `VICTORY_METAL_OUT` from `tools/mac/run.local.env`; none: an error that names the three ways out (set it, write `run.local.env`,
  `--backend d3dmetal`). **`tools/mac/run.local.env`** is untracked (in `.gitignore`), sourced at the start if present, the environment
  wins over it; it holds machine-specific values (`VICTORY_METAL_OUT`, `VICTORY_METAL_ARCHIVE`, `VICTORY_GAME_LOCK`).
- **Game lock** (`tools/mac/game-lock.sh`, taken by `run.sh` before any `victory.exe`, not by `--print-env`): one game instance on the
  machine at a time, shared by all worktrees; queueing, stale-lock takeover, release on end/timeout/interrupt; `--allow-concurrent`
  needs `--i-have-approval`; `check.sh` runs niced while it is held; each run logs the load average (`load.log`). See `docs/mac/RUN.md`.
  *Remove when* never needed: this is the rule for the machine, not a workaround.
- **Backend** (`--backend metal|d3dmetal`) is the only difference between two runs, besides the data directories (below): the prefix (`target/wineprefix-livetr-d3dmetal`
  or `-metal`, never shared), the dlls in its `system32` (the Metal ones are installed with the originals kept as `.metal-orig`,
  and put back when the D3DMetal backend is chosen), `WINEDLLPATH`, `WINEDLLOVERRIDES`, `VICTORY_METAL_LOG`,
  `VICTORY_METAL_NOTIMPL_LOG`. `run.sh --print-env --backend d3dmetal` against `--backend metal` (or no option) shows exactly that.
- **Profile** (`--profile fidelity|perf`, default **perf** for now, see "Profiles and when to switch" below).
- Everything else is set the same way for both: the helper DXC (G4: `VICTORY_DXC_HELPER`, `VICTORY_DXC_DIR`, `DXC_LIB`), the input
  (I1: `VICTORY_KEYS` 8-3600/3:Enter, `VICTORY_INPUT_REPLAY` the neutral input), `VICTORY_FAULT_TRACKING` (1), the masked-store
  patch (follows tracking, not set), `VICTORY_PRESENT_MAX` (P1), `VICTORY_POOL_CHECK` (L3), `VICTORY_LOG_RENDER_SIZE` (P2),
  `WINEPREFIX` (explicit; only that prefix's wineserver is ever stopped), the data, cache and log directories, recording and
  frame capture options, `--sample` (memory/GPU lines in `mem.csv`).
- **Data directories**: `data/livetr-<profile>-<backend>`, cloned from the read-only seed `data/livetr`; `--fresh-data` clones
  again. Both backends start from the same state and can run in parallel. These directories and the prefix are the allowed differences
  of `--print-env` between backends, besides `WINEDLLPATH`/`WINEDLLOVERRIDES`/`VICTORY_METAL_LOG`/`VICTORY_METAL_NOTIMPL_LOG`.
- **Capture and watchdog switches**: `--record-at` (`VICTORY_RECORD_AT`, I2), `--capture-frames` (takes `w<N>`, I2), `--no-watchdog`
  (`VICTORY_MAC_NO_WATCHDOG=1`, L5; logged as `non-fidelity`) and the preset `--corpus` (`--record-at w<N>` plus `--no-watchdog`, default
  `w5`): see "Capture runs" in `docs/mac/RUN.md`.
- **Stop as soon as you have the data** (`--until-capture`, `--until-log ERE`, `--until-world N`, `--until-present N`, `--linger SEC`;
  `tools/mac/until-watch.sh`, tested by `tools/mac/test-run-until.sh` with a fake Wine): a watcher next to the title polls its logs (and
  the capture/recording folders) once a second and, when any condition holds, ends the run the way `--timeout` does (title stopped,
  `wineserver -k` of the run's prefix, lock let go), exit status 0, `until 条件满足：<which> 在 <s> s` in `start.txt`/`run-env.txt`/
  `stderr.log`. `--timeout` is then only the cap: not met, the log says `until 未满足` and the exit status is **6**. Without
  `--until-*` the run is as before. One Rust line pair for `--until-world` (I2 below): `VICTORY_LOG_WORLD=1` makes the title log every
  world frame (off by default). The same change fixes the time-limit sleeper, which logged `== time limit ... reached` (and could
  signal a stale pid) when a run ended before its limit. *Remove when* never needed (it is run.sh's own interface).
- `--print-env` prints what would be passed (sorted, `~` for the home directory) and the command, and does nothing else.
- Each run keeps `run-env.txt`, `start.txt`, `summary.txt` next to its logs.
- Refuses: the Metal backend on a D3DMetal prefix (`wineprefix-play`, `wineprefix-livetr`, `wineprefix-livetr-d3dmetal`); a start while
  another run holds the game lock (it queues instead; two games at once ran a 32 GB Mac out of GPU memory; `--allow-concurrent` only
  with `--i-have-approval`).

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
- Rosetta (fixed 2026-11-28, `mac: T titlewrite ...`): `titlewrite::decode::stores_do_what_the_processor_does` compared the flags of
  `lock cmpxchg [r11], dl` with the decoder's (0x881 against Rosetta's 0x80). The decoder was right (flags = accumulator - memory,
  as x86); Rosetta computes the flags of a miss from memory - accumulator (ZF right) and clears bits 63:32 of rax on a 32-bit hit.
  The test now holds the decoder to what the processor's own `cmp` gives (Rosetta does that right), and the processor's `cmpxchg`
  only to ZF, memory, rdx and the low bits of rax. Passes under Wine; with the decoder's operands swapped it fails.
  Exposure of the title: `tools/mac/census-cmpxchg-flags.sh` walks every path after all 28,305 `lock cmpxchg [mem]` sites until
  CF/SF/OF/PF are overwritten or the path ends: 0 readers, 0 unresolved (so Rosetta's wrong flags of a miss are never seen).
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

`victory-hle` (2026-11-28, `tools/mac/wine-tests.sh -p victory-hle`; 181 pass, 4 fail, 2 ignored of 187 before the 2 new `files`
tests and the 11 `mac_counts` ones were in): all of `xmem` (but one), `files` (7), `dstorage` (18; 1 benchmark ignored) pass, ten
runs of each. Not passing:
- `xmem::a_title_thread_goes_on_using_the_rest_of_a_view`: intermittent (about 1 in 10 inside the xmem suite, 0 of 32 alone): the
  title thread reads its page back as 0 or as bytes up to 99 writes old while the other pages of the view are unmapped and mapped
  again. `unmap()` returns the whole view to a placeholder and maps the rest again, so the page is a placeholder for a moment;
  on Windows it faults and waits, here it did not fault and the writes were lost. Matrix: `mac.hle.xmem_map_partial_view`.
- Not xmem, not explained here: `net::only_loopback_is_reachable` (Wine's ws2_32 has no `WSAConnectByList`),
  `xgameui::dialogs_end_with_what_was_typed` (a different HRESULT on the host: -2147483638 for -2147467260),
  `xgamesave::a_kept_blob_past_the_link_limit_is_copied` (the temp folder is not on NTFS, 1024 links to a file are not there).

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
input script and Enter keys as `run.sh` has them; both through `run.sh --backend metal` (default; `--backend d3dmetal` is an option of the script) `--perf|--fidelity --data --cache --out`,
and `victory warm` through `run.sh --exec -- warm`, so the prefix and the environment are `run.sh`'s) and prints the share of each class over the first N world frames. *Remove when* the Metal backend replaces the D3DMetal path.

### G9 — the console device's pipeline cache methods, slots 0x1c8 and 0x218 (`victory-gpu/src/mac/pso_x.rs`)

*What the two slots are* (decoded from the title, 1.0.2.1; call sites title+0x2477578 and title+0x24800d5; the
Windows line has them as `E_NOTIMPL` too, `device.rs` `unknown_pipeline`). Both are **hits of the title's own pipeline
cache** for a pipeline it can also make from the shader containers it loads, so a `E_NOTIMPL` is a cache miss, not a lost
draw. The cache object (global at title+0x75ddb90) is filled from a file at start. 0x1c8: a loop over the graphics
entries calls it once each and keeps the pipelines in a table by the first shader's key; the title asks that table
before it builds a stream (title+0x2479f40) and calls slot 0x2b0 (title+0x2479a87) only on a miss. 0x218: one call per
compute pipeline job (title+0x24800d5, key = the job's); on failure the job calls slot 0x1f0 with the compute container
it has loaded (title+0x247fe66). Both descriptions start as 0x50/0x58 do (root signature, then (pointer, size) pairs),
but the pairs are the compiler's `FSPX` entries and code in GPU memory, not bytecode. The compute record (12 qwords: root,
packet 0x1c, hash 0x14, code, metadata 0x18+32n, two pointers, a 64-bit hash) is read from sizes and the log of one run, not
from a whole sample; the graphics record (root, at least six pairs: 0x184 and 0xbc-ish blocks around code, then per-stage
blocks) is not decoded.

*What is done.* `VICTORY_MAC_PSO_X=1` (off by default until a batch run has compared it): slot 0x218 builds an `FSPX`
part from the record, wraps it in a container with one `ACCC` part (where the shared layer already keeps compute programs of
that form), checks that `victory_shader::native_programs` finds one compute program in it, and hands out the pipeline
(`pipeline::compute_from_container`, creation method `compute@0x218` in the G8 tables). Anything that does not read or
parse is `E_NOTIMPL` as before, counted (`mac G9: slot 0x218 record not taken`, once per reason; the G8 `notimpl[... 0x218=n]`
counts those only), and the title makes the pipeline from its container. Slot 0x1c8 stays `E_NOTIMPL` on purpose: a record
has no signature parts (`ISG1`/`OSG1`/`PSV0`) that the graphics translation reads, and the title's stream path is complete.
`VICTORY_MAC_DUMP_NOTIMPL_PSO=<dir>` writes every different call of either slot: `0x1c8|0x218/<id>.txt` (qwords, what each
was taken for, our root signature object's bytes and blobs) and `blobs/<hash>.bin` (the memory each (pointer, size) pair
names, once per content; `_BYTES`=256, `_FOLLOW`=262144, `_MAX`=60000 tune it). The id hashes the non-pointer words and the
memory behind the pointers, so the same pipeline in another run or place is the same file.

*Hooks in upstream files:* `device.rs` (`precompiled_compute` for slot 0x218, one dump line in `unknown_pipeline`),
`pipeline.rs` (`compute_from_container`, `PIPELINE_IIDS` made `pub(crate)`). *Remove when* the Windows line implements the
slots (the records' format is then theirs), or when a batch run shows the title's own fallback is as good and the patch is of no use.

### G10 — a "read" fault in a guarded block is a write (`victory-gpu/src/mac/rosetta.rs`, 1 hook in `watch.rs`' handler)

*Question (translation map `mac.isa.rmw_fault_access_type`).* Under Wine on Rosetta a locked or exchanging read-modify-write
(`lock or/bts/cmpxchg/xadd`, `xchg`, 8/16/32/64-bit, 56,796 sites in the title) that faults on `PAGE_NOACCESS` memory, or that
straddles a page boundary into a `PAGE_READONLY` page, reports access type *read* to the VEH; x86 reports a write
(probe: `lock_or32`, `xchg32`, `lock_xadd*`, `cmpxchg*`, `lock_cmpxchg*`: `fault`, `cross`, `crossro`, `crossfirst` modes FAIL
`access_type`; the same instructions in the `ro` mode, wholly inside a `PAGE_READONLY` page, report a write and pass).
`watch::on_write` opens a guarded block only for a write, so a misreported one would reach the crash reporter.

*What the guards use.* `PAGE_READONLY`, never `PAGE_NOACCESS`: `protect_alias` and `protect_here` in `watch.rs` (and `inplace.rs`'
own guards) call `VirtualProtect(.., PAGE_READONLY)`, and `unguard`/`open` put `PAGE_READWRITE` back. `PAGE_NOACCESS` appears
only for the title's own reservations/placeholders (`xmem.rs`), which the handler is not meant to open. So the plain case
(an RMW wholly inside a guarded block) is the `ro` mode: a write, already handled. Left are RMWs that straddle from open
memory into a guarded block. Guard blocks are 64 KiB aligned.

*Measured.* On the mapped views the guards protect, `lock or`, `xchg`, `lock xadd`, `lock cmpxchg` (dword) starting 0-3 bytes before a
guarded block's start report a **write** at the block's first byte, with or without this patch (test below with a recording
handler, 2026-11-28); the probe's misreport is on private `VirtualAlloc` memory at 4 KiB and 16 KiB-aligned boundaries
alike (`b=0x3000/0x4000/0x8000`), which the guards never cover. So no run is known to have hit it.

*Patch (small, defensive).* `read_fault_is_write(rosetta, access, guarded)`: under Rosetta a fault of access type read in a
block that `PROTECTED` holds is taken as the write it is (the handler then opens the block and the instruction runs again).
Why this one and not the alternatives: `PAGE_READONLY` is already what is used, so "switch to READONLY" is a no-op; switching
guards to anything else would change what every title store sees. A read fault on a `PAGE_READONLY` page cannot be a real read
(reads are allowed), so inside our own guarded blocks the rule has no false positive; a block that was unmapped meanwhile fails
`open` as before and falls through to the old paths. Nothing changes off Rosetta. Tests:
`mac::rosetta::tests::a_read_fault_in_a_guarded_block_is_a_write_only_under_rosetta` (the rule, native),
`watch::tests::locked_read_modify_writes_into_a_guarded_block_complete` (Windows; run under Wine/Rosetta: eight locked/exchanging
forms into and across a guarded block all complete). Not needed on Windows (the CPU reports a write). *Remove with G2*, or
when Rosetta reports these as writes; keep until a probe on a macOS update shows `crossro` passing.

### L1 — masked 256-bit stores rewritten (commit `781672bc`, `victory-loader/src/mac/masked.rs`)

AVX masked stores (`vpmaskmovd/q`, `vmaskmovps/pd`, `vcvtps2ph` to memory) redone as plain stores
Rosetta can resume. Done **by default under Rosetta, with fault tracking on or off** (11-28: a masked store whose
lane >= 1 faults is fatal under Rosetta whatever put the guard there; the patch costs a jump per site and nothing
when no fault happens), so it needs no variable: a start with `VICTORY_PATCH_MASKED` unset (`tools/macos/victory.sh`,
a plain `wine victory.exe`, and `tools/mac/run.sh`, which no longer sets it) logs
`masked stores  patch on (default under Rosetta, tracking on; all + the 8 extra sites of build 1.0.2.1)` and then
`masked stores  1241 patched to plain stores`. Off Rosetta nothing is patched
(`masked stores  patch off (not under Rosetta, ...)`).
`VICTORY_PATCH_MASKED`: `none` (no patch), `all`, or hex RVAs (explicit, taken as given). `masked::decide`
(`victory-loader/src/mac/masked.rs`; `apps/victory/src/mac.rs` only calls it and prints its `why`) adds the 8 plain
256-bit stores of #144 (RVAs in `BUILD_1_0_2_1_EXTRA`, build 1.0.2.1, hash `b99f876b…`) when the
variable is unset **or is plain `all`**, under Rosetta: 1233 found by the scan + 8 = 1241. Before, an explicit `all` (which `run.sh`
exported) replaced the list, so those 8 were never patched on a `run.sh` start; and until 11-28 (`4ab34ad8` made it follow
tracking, which is on by default) `VICTORY_FAULT_TRACKING=0` patched nothing.
*Tests* (native, `masked::patch_tests`): `plan_patches_by_default_under_rosetta_whatever_tracking_says` (the four
combinations of Rosetta and tracking), `plan_takes_the_variable_as_it_is_and_the_extra_sites_only_for_all` (none / all /
RVAs / another build), `the_extra_sites_are_all_plus_eight_distinct_rvas`.
Not needed on Windows: it resumes them. Remove with G2.

*Coverage (checked 2026-11-28 against the static census of build 1.0.2.1, `research/census/title-isa-20261127`;
`cargo run --release -p victory-loader --example masked_census -- <image> <site lists>`).* The census (a real
disassembly) has 1130 masked stores (`vpmaskmovd` 789, `vpmaskmovq` 160, `vmaskmovps` 180, `vmaskmovpd` 1), 70 `vcvtps2ph [mem]`
and 33 `vextract{f,i}128 [mem]`, 1233 sites. The scan finds 1233, **the same 1233: 0 missed in each of the three classes**, and
`mac::masked::audit` decodes each one and builds its trampoline (0 undecoded, 0 out of reach, none with a segment or `67` prefix
in front of the VEX byte, which the trampoline's `lea` would not honour). The scan used to find **1234**: the extra one was a byte
look-alike, `c4 c3 6d 39 b9` inside the immediate of `mov eax, 0x396dc3c4` at RVA `0x2d566d5`, taken for a `vextracti128` and so *patched*: its five bytes overwritten by a jump, which changed that constant
and the `mov ecx, 0xae64642c` after it. A real `vextract*128`/`vcvtps2ph` has VEX.vvvv = 1111; the scan now requires it
(test `the_unmasked_stores_need_vvvv_1111`), so the count is 1233 and 1241 with the 8 plain stores. `vcvtps2ph [mem]` is in this
patch since the start (the 70 sites above); there is no separate patch for it (see "L1 and vcvtps2ph" below).

*L1 and `vcvtps2ph`.* The Wine probe (2026-11-27, `docs/mac/translation-map-mac.md`) found `vcvtps2ph [mem], ymm/xmm, imm`
fatal under Rosetta (EmulateForward, the VEH never runs) on **any** write fault of the store, 60 of 60 runs; the title has 70.
They are not missing a patch: L1's scan has had them from the start (`MaskedKind::Vcvtps2ph`; the trampoline runs the same
`vcvtps2ph` into `[rsp]` with the same imm8 and the same MXCSR, then moves the 8 or 16 bytes out with plain `mov`s, which
Rosetta resumes), and the census above has all 70 of the census's 70 in it. What was missing was the proof for them:
`a_patched_vcvtps2ph_resumes_after_a_write_fault` (Windows test, run under Wine/Rosetta by `tools/mac/wine-tests.sh -p victory-loader
--lib mac::masked`) stores ymm and xmm half-floats (imm8 0 and 3) inside a read-only page and across the boundary into it at five
offsets, with a handler that makes the page writable, and every one resumes with the right bytes (the same cases unpatched
are the probe's fatal ones); `patched_stores_match_the_stores_in_every_form` already compares the patched store with the
real one (memory, registers, flags) over 4000 random operand forms, vcvtps2ph with random imm8 among them. So no L6
for it. *Remove with L1.*

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
- Path line (11-28), once, before any site is written (`resolution::path_line`, planning is `resolution::plan_asked`): a fidelity run
  (nothing asked: `VICTORY_OUTPUT=0`, no pool check, scale or upscaler switch) logs `resolution: fidelity, no site patched` and
  writes nothing, not even the divide clamp; any patch asked logs `resolution: perf, N sites asked to be patched (...)`. Tests
  (native): `a_fidelity_run_patches_no_site_and_says_so`, `a_perf_run_patches_the_output_with_the_pool_check_and_the_clamp`,
  `the_pool_check_alone_the_scale_and_the_upscaler_are_each_a_perf_patch`.
- Which settings decide the output: see "Which `settings.xml` keys decide the output" below.

Remove when the Mac GPU path is fast enough for the title's own size.

### L4 — `clwb` loops become NOPs under Rosetta (`mac/clwb.rs`)

`clwb` faults under Rosetta (one exception, about 300 µs through Wine, per loop; the world load
crawled, #135). The Windows line already emulates `clwb` loops in `crash.rs` (one exception per
loop), fine on a PC but too slow through Wine, so the 8 loops of the supported build are patched
out at load (nothing is patched unless exactly 8 are found). The loop scan is a copy of
`crash.rs`'s `clwb_loop` (test included) so `crash.rs` is untouched; if the Windows line changes
the pattern, change the copy. `VICTORY_NOP_CLWB=0/1`. Remove with G2, or if Rosetta handles `clwb`.
*Log (11-28).* `clwb: 8 loops replaced` (the count is the number patched; exactly 8 are required). Any other count
patches nothing and logs `clwb: 0 loops replaced (found N, expected 8: not the build they were counted in, nothing
patched)`; off (not Rosetta or `VICTORY_NOP_CLWB=0`): `clwb: 0 loops replaced (left as they are: ...)`. *Tests*
(native, `clwb::tests`): `eight_loops_are_replaced_and_logged_with_the_count` (8 loops in two sections, 4- and 5-byte
forms, become NOPs of their length and the `add`/`cmp`/`jbe` stay), `any_other_count_patches_nothing_and_says_what_it_found`
(0, 7, 9), `the_patch_is_wanted_under_rosetta_and_the_variable_decides`.

### L5 — the title's hang watchdog off, `VICTORY_MAC_NO_WATCHDOG=1` (`mac/watchdog.rs`, hook in `apps/victory/src/mac.rs`)

**Off by default; not fidelity.** With `VICTORY_MAC_NO_WATCHDOG=1` (anything but empty or `0`) the three jumps to the
watchdog's crash path are made two-byte NOPs (`66 90`) at load, and the log gets one line:
`non-fidelity: title hang watchdog disabled` (stdout and `tracing`). Without the variable nothing is read or patched.

*What it is.* The title's own watchdog (see "Title crash at 0x2215c35" under G7): once a second it checks the heartbeat
(`title+0x22149f0`) of each registered thread against a limit, and counts 30 checks (`ebp = 0x1e`) without a
finished main-thread frame; when either fails it writes `[0]` on purpose (`mov dword ptr [0], 0` at +0x2215c35,
then `int3`). A recording run, `VICTORY_CAPTURE_FRAMES` or the Metal path make world frames take many seconds, and
the title kills itself in the world, so a long capture run cannot finish. Ported from VIctory-macOS `origin/main`
commit `a38f26d9` (`VICTORY_NO_HANG_WATCHDOG`, `hang_watchdog` of `KnownBuild`): three sites, build 1.0.2.1.

*Sites (read again from the exe's bytes with a disassembler, 2026-11-27).*

| site | bytes | jump |
|---|---|---|
| `+0x2215bdc` | `73 2e` → `66 90` | `jae` after `vucomiss xmm8, xmm7` (heartbeat age vs. limit), to the crash path `+0x2215c0c` |
| `+0x2215c70` | `74 04` → `66 90` | `je` after `dec ebp` (30 checks without a frame), to `+0x2215c76`, on to the crash path |
| `+0x2215d84` | `74 04` → `66 90` | the same count-down in the loop's other form, to `+0x2215d8a` (reporting call, then the crash) |

*Safety.* Only when the exe's BLAKE3 is the Series 1.0.2.1 one (`b99f876b…`, as L1's extra sites) and all three
sites hold the expected bytes: compared first, all of them, and nothing is written on any mismatch. Otherwise the
watchdog stays on and the log says why (`title hang watchdog left on: title+0x… holds … where … was expected`, or
`build … is not Series 1.0.2.1`). Unit tests (`mac/watchdog.rs`) run on a synthetic image, not on the exe.

*Fidelity.* The game's logic, rendering and timing are untouched; the only change is that the title no longer
terminates itself when it is slow. Anything that needs the watchdog to fire (a real hang) now hangs instead of
crashing, so a run with it on can stall for good: give such runs a wall-clock limit outside the game. Frames
captured with it on are as faithful as without; the log line above marks the run so its numbers are not
mistaken for a plain run's.

*Launcher.* Not set by `tools/mac/run.sh` (another change owns that file); export the variable by hand or add it
there when the capture runs need it.

*Remove when* frames are fast enough that the watchdog never fires (the Metal backend, or a measured run with a
world frame under a second shows it quiet), or when the Windows line gets a switch of its own.

### L6 — 16-bit `lock cmpxchg` check: `VICTORY_MAC_CMPXCHG16_CHECK=1`, off by default (`victory-loader/src/mac/cmpxchg16.rs`, 8 lines in `apps/victory/src/mac.rs`)

*Question (Wine probe 2026-11-28, `lock_cmpxchg16_hit`).* Under Rosetta a `lock cmpxchg [m], r16` whose address is 15 mod 16
loses the update when the compare **succeeds** (memory unchanged, ZF set); a miss, every other offset, the 32/64-bit forms and the
unlocked form are right. The title has 2036 16-bit `cmpxchg`, all locked (`python3 tools/mac/census-cmpxchg.py.txt <out dir> < objdump text`, results in
`research/census/title-cmpxchg-20261128/`): 1877 `[base+even disp]` (63 of them rsp/rbp), 26 `[base+2*index+even disp]`, 70
RIP-relative whose targets are 0, 2, 4 or 8 mod 16 (none 15). A hit needs an **odd** address: a 16-bit atomic in a packed
object, or a base pointer that is odd. Nothing static says so, and no compiler makes an atomic 16-bit field odd on its own.
Not provable from the bytes: so a run decides.

*The check.* With the variable set to `1`, every one of the 2036 sites (the scan is bytes, so it works on any build; the
scan equals the census: `cargo run --release -p victory-loader --example cmpxchg16_census -- <image> <tsv>`) becomes a jump to
a trampoline that saves rcx and the flags, counts the run in its own slot, computes `lea rcx,[operand]`, and if the low four bits
are 1111 records the address and counts a hit; then it restores rcx and the flags and runs the original instruction (copied, its
RIP-relative displacement redone), and jumps back. A thread prints every 5 s while the numbers change (and every minute):
`cmpxchg16  N of 2036 sites ran, M met an address = 15 mod 16` plus the first 8 hits (RVA, hits of runs, last address). It does not
change what the instruction does. Test: `the_checked_instruction_does_what_the_instruction_does` (Windows, run under Wine/Rosetta
by `tools/mac/wine-tests.sh -p victory-loader --lib mac::`): three operand forms at nine offsets 0..4095 including 15, 31, 4095, hit
and miss: AX, ZF and memory are the instruction's, and only the 15 mod 16 runs are counted as hits (in the test the
instruction's own bug may also show at those: it compares patched with unpatched, both through Rosetta, so both are equally wrong;
this is a *counter*, not a fix).

*If it ever reports a hit.* Rewrite that site: take the address, and do the compare-and-swap with a 32-bit `lock cmpxchg` loop on the
dword that holds the word (at `addr-2`, which is 13 mod 16, and 32-bit forms are right at every offset): load the dword, check the
word in its upper half equals AX, build the new dword, `lock cmpxchg`, retry on a changed dword, set ZF and AX as the instruction does
(AX = the old word on a miss). It needs rax/rdx/rcx/r8 saved and a RIP-relative redo, in the style of this module's trampoline: about
half a day with the same random-form test against the instruction (on a Mac the instruction's result at 15 mod 16 is the wrong one,
so the reference is a soft model of cmpxchg). Do it only when the check reports a hit; until then no behaviour is changed.
*Remove when* Rosetta fixes it (re-run `lock_cmpxchg16_hit`) or after a few full sessions of the check with no hit.

*The 32-bit form (`mac.isa.cmpxchg32_rax_hit`).* The probe also showed that after a **hit** of a 32-bit `lock cmpxchg` Rosetta **zeroes**
the upper half of RAX, where x86 leaves RAX alone on a hit (only a miss writes EAX). Only code whose RAX has a non-zero upper half before
the instruction and that reads all of RAX after a hit, before writing it, can notice.
Of the title's 9665 sites (all locked), 834 do read RAX that way on the hit path; the upper half was provably zero before for 772
(an earlier `mov eax,..`) and for 57 more across a branch; the one checked by hand in 117759 is safe (`mov eax,[rax]` in the loop);
the others are false positives of the classifier (3-operand `imul`, `cdqe`, `nop [rax]`) or `ret` after a CAS loop that loaded EAX with
a 32-bit load. Three remain which the linear analysis could not decide (RVAs `0x3e00a2`, `0x2bdb621`, `0x335a93d`; the first two are
CAS loops whose eax comes from a 32-bit `mov eax,[..]`, the third `cmpxchg [rcx+0x14], edx` followed by `vmovd xmm1, eax`, which
reads only EAX): no exposure found, no patch.

### L7 — a dump of the machine at the title's own `int3` assertions: `VICTORY_MAC_INT3_DUMP=<rva>[,<rva>...]` (`victory-loader/src/mac/int3dump.rs`, 7 lines in `crash.rs`; `tools/mac/collect-title-crash.sh`, 1 line in `tools/mac/run.sh`)

*Class: **diagnostic, not fidelity, off by default.** It only reads memory and prints; the title's path after the trap is the one it had
(the vectored handler still returns `EXCEPTION_CONTINUE_SEARCH`).*

*Question (`mac.title.int3_self_exit_r6`, `mac.title.srl_prestream_stall`).* The title's BUILDING pool (22500 slots) is full at
image+0x5149aec in about one long run in four, 350-490 s after the start. Is the pool full because its nodes are **held** (a leak, or a
Prestream that never finishes), or because its **free list was damaged** (e.g. a `lock cmpxchg` update lost under Rosetta, see L1, L2, L6)?
The title's own crash.log gives the use counts only, and `--new-game` replaces the data directory (and the log with it).

*The switch.* `VICTORY_MAC_INT3_DUMP=0x5149aec` (hex RVAs, `0x` optional, comma separated; an item that is not hex is named and ignored;
unset or empty = off). The vectored handler, after printing its usual first-chance report, checks a breakpoint exception (`0x80000003`):
if its address (or the context's `rip`, or `rip-1`) is `module base + a listed RVA`, up to **4 dumps per process** are printed to stderr and
appended to the loader's crash log, every line starting `VIctory int3-dump:` (`grep 'int3-dump' stderr.log`):

1. all general registers, `rip` and `eflags`;
2. `[rcx-0x40, rcx+0x100)` as qwords in hex, four to a line with the offset from `rcx` (`????????????????` for a qword that cannot be read);
3. the pool at `rcx`: header (base `[rcx]`, step `[rcx+8]`, capacity `[rcx+0xc]`, used counter `[rcx+0x10]`, head `[rcx+0x20]`, next array `[rcx+0x28]`,
   allocated mark `[rcx+0x30]`); the first 64 nodes (`base + i*step`, 32 bytes each, with their next entry); the free list from the head, at most
   64 steps listed and the whole chain counted (stopping, with the reason, at a cycle, an index >= capacity, or a node marked allocated);
   a census of the whole next array (slots marked allocated / not / outside the pool / unreadable) and a **verdict line**;
4. the `rip` of every thread (suspended one at a time just long enough to read its context, into room made beforehand; named as `module+rva`),
   the trapping one marked.

*Where the free list's `next` is* (confirmed in the disassembly of `Alloc` 0x145149980, its constructor 0x1416cbe00 and `Free` 0x1451f2801 of
build 1.0.2.1): **not in the node.** `next[i]` is a `u32` array at `[rcx+0x28]`; `Alloc` pops `head & 0x1ffff = i`, writes `head = (head & 0xffffffff00000000) | next[i] & 0x1ffff`
(`lock cmpxchg [rcx+0x20]`), `lock inc [rcx+0x10]` and ors `[rcx+0x30]` (0x1ffff) into `next[i]`: **an index field of 0x1ffff in `next[]` means "allocated"**, 0x1fffe ends
the list and is the head's "empty". `Free` puts the old head index into `next[i]` (the bits above the index are a counter), `lock dec [rcx+0x10]`.
The trap at 0x145149aec is also reached when `[rcx] == 0` (no node memory); the dump says so.

*Reading the verdict:* `EXHAUSTED` (head empty, every slot marked allocated, used counter = capacity) = the nodes are held: look for who does not give them back
(the Prestream node, `mac.title.srl_prestream_stall`); `FREE LIST LOST` (head empty, K slots not marked allocated) = a damaged list, then the next runs are with
`VICTORY_MAC_CMPXCHG16_CHECK=1` (L6) and without the 256-bit store patches (L1, L2) to see which one does it; `LIST INTACT` / `LIST BROKEN` = the head is not empty
(trap from `[rcx]==0` or a free after the trap) / the chain is not the set of unmarked slots. The dump is not atomic: frees from other threads that land during it show as small differences (counter vs marks).

*Tests* (`cargo test -p victory-loader --lib mac::int3dump`, native): the RVA list (prefix, duplicates, bad items), the hit test, the register and memory formatting (offsets, unreadable
qwords, `rcx` near 0), the pool analysis on **fake memory buffers** (exhausted pool, lost free list, whole list with counters above the index, a 200-node list walked 64 steps but counted
whole, cycle, index outside the pool, held node on the list, `[rcx] == 0`, implausible header, a missing part of the next array), the thread lines, and the order of the whole dump.
Two Windows-only tests (`windows_tests`: the process reading its own memory and refusing unmapped addresses; a pool built in memory) compile with the cross build and are for `tools/mac/wine-tests.sh -p victory-loader --lib mac::int3dump` (not run yet).
Memory is read with `ReadProcessMemory` on the own process, so an unreadable address is a `false`, not a second fault in the handler; a thread that traps inside the dump does not start another.

*`tools/mac/collect-title-crash.sh DATA OUT [SINCE]`.* `run.sh` calls it once after `end.txt` is written: it looks for the title's own crash log under the run's data directory
(`grep -rla 'Game State'`, newer than `start.txt`) and copies it to `$out/title-crash/`, so BUILDING/OBJECT/INSTANCE_LIST use, the SRL state and `Streaming Requests` survive the next `--new-game`.
It never fails the run.

*Remove when* `mac.title.int3_self_exit_r6` and `mac.title.srl_prestream_stall` are closed (the cause is found and fixed or the node is proved harmless); the module is self-contained.

### G11 — `VICTORY_MAC_LAYOUT_STATS=1`: which texture layouts the title creates (`victory-gpu/src/mac/layout_stats.rs`, 1 hook in `resource.rs` `hand_out_resource`)

*What.* Counts every texture the title creates by dimension, layout field (`0x100 | swizzle mode`), render-target/depth flags,
element size, block, and whether it has mips, depth/array, or samples; logs one `layout stats:` line per distinct key when
the texture count reaches 256, 512, ... and the number of textures whose mode the detiler cannot address (XOR modes `_T`/`_X`).
Off unless set (one atomic load per texture). Nothing the title or the host sees changes.

*Why.* Map rows C-V-003 (detile) and C-V-005 (tile modes): the detiler is proven against addrlib for the modes without XOR
(`victory-formats/tests/addrlib_golden.rs`), and for sizes of the XOR modes; which modes the title really uses was never
counted (the capture `frame.bin` holds host-side descriptions only). The count decides whether XOR addressing (needs the
per-surface pipe-bank XOR) is worth writing.

*Why not needed on Windows.* The Windows line has the same detiler and the same question; this is a measurement, not a fix. *Remove when*
the census has been read and the answer is in the map row.

### G12 — test-only hooks and a 3D UAV census for the shared-layer proofs (`victory-gpu/src/mac/prove/`, `#[path]` test modules in `descriptor.rs`, `descriptor/host.rs`, `resource.rs`; `pub(crate)` on `RESOURCE_IIDS`, `QUEUE_IIDS`, `ALLOCATOR_IIDS`, `QueueDesc` fields; `queue::signal_offset_for_tests`; `layout_stats::uav_3d` + 1 hook in `descriptor.rs` create_uav)

*What.* Tests, nothing at run time: `descriptor_records` (CopyDescriptors/Simple, view records, increments), `descriptor_views` (GPU reads of SRV/UAV/cube/null/constant views on the host device against the D3D12 rules),
`queue_fence` (fences, events, queue Signal/Wait), `resource_footprints` and `resource_heaps` (GetCopyableFootprints / GetResourceAllocationInfo against a model of the spec and the host device; placed/committed/Map rules). They are child
modules (`#[path = "mac/prove/..."]`) so they reach private items; the visibility widening above is for the tests only. The one run-time part is the 3D unordered-access census (`VICTORY_MAC_LAYOUT_STATS=1`): counts the
title's 3D UAVs with a first depth slice other than 0 or a size other than the rest (D3DMetal cannot make partial 3D views, finding F1 of C-V-012).
Shader side: `victory-shader/tests/sampler_spec.rs` (S-G-009) and `emit/mac_dxil_check.rs` (S-D-001: a hook in `emit_program`, `cfg(all(test, not(windows)))`, that checks every DXIL container the translator's tests build with an independent container reader and `llvm-bcanalyzer`; Mac has no `dxil.dll`).

One behaviour change (the only one outside tests): **shader cache entries are sealed** (`shader_cache.rs` `seal`/`unseal`, 16 bytes of BLAKE3 over the key (build + container) and the encoding, appended to each entry; an entry without a good trailer is a miss and is translated again). Before, a flipped byte inside an entry's DXIL decoded as a valid, different shader and was served; so was an intact entry under another key's name. All entries of earlier builds are misses once (the cache refills). Tests: `mac/prove/shader_cache_damage.rs`.
`mac/prove/device_slots.rs` (C-V-001): slots 0x1c8/0x218 fail cleanly with `out` null when the switch is off.

Second batch (branch mac/shared-prove-2): `victory-shader/src/gfx10/llvm_tests.rs` + `tests/data/gfx10_llvm_gfx10{10,30}_{0..3}.txt` (S-I-001, generated by `tools/mac/llvm-isa-oracle/gen.py` from LLVM's AMDGPU disassembler: mnemonics and lengths only, no game data), `mac/prove/{device_features,root_flags}.rs` (shared.device.feature_query, shared.root.signature).
`mac/prove/pipeline_stream.rs` (C-V-009; `pipeline.rs`'s stream loop is now `walk_stream`, no change of behaviour) and `victory-shader/src/mac_prove_hints.rs` (S-I-002).
One log line (`device.rs` check_feature_support): the first time the title queries each feature id, at info level: it tells whether the title reads the options it is given (rows shared.device.options_*; the values themselves are not changed: no evidence the title branches on them, and a change cannot be verified without a game run).
**Second behaviour change: a blocking `ID3D12Fence::SetEventOnCompletion(value, NULL)` waits** (`fence.rs`, was E_NOTIMPL at once; polls the value, 10 s at most (`BLOCKING_WAIT_MS`), then E_FAIL with a log line instead of hanging); `object.rs` gains `E_FAIL`. Test: `queue_fence::a_blocking_wait_returns_when_another_thread_signals`.

*Why.* Map nodes C-V-001/008/012/014/017, S-G-009, S-D-001 and api.CreateConstantBufferView/Resource::Map were "used, not proven"; second batch: S-I-001, shared.fence.wait, shared.device.feature_query, shared.root.signature.

*Why not needed on Windows.* The Windows line has its own tests of these modules; these add spec-referenced ones. *Remove when* the Windows line adopts equivalent proofs; the census when F1 is answered.

### H1 — `SuspendThread` only for the title's own threads (commit `5050245c`)

New `victory-hle/src/modules/mac.rs`, registered in `modules/mod.rs` **only when `rosetta()`**,
plus `victory_loader::mac::in_title_image` (`threads.rs`, 8 lines). Under Wine on Rosetta a thread
of ours suspended while Wine held its virtual-memory lock left the lock held for good (#50).
Remove when CrossOver Wine fixes the lock (retest after a Wine bump from 26.3) or with G2.

### H2 — `dbghelp.dll` stubbed instead of passed to the host

`host.rs` no longer allows `dbghelp.dll`; new `victory-hle/src/modules/dbghelp.rs` answers for it
(`MiniDumpWriteDump`, `SymInitialize(W)`, `SymCleanup`, `SymSetOptions`/`SymGetOptions`, `SymFromAddr(W)`,
`SymGetLineFromAddr64`: all fail with `ERROR_NOT_SUPPORTED`, none reads a caller pointer;
`MiniDumpWriteDump` logs "minidump skipped (dbghelp stubbed)"). The title imports nothing from dbghelp
statically: it loads the DLL by name and looks up only `MiniDumpWriteDump`.

*Why.* After the title's first fatal exception its crash handler calls `MiniDumpWriteDump`; Wine's
dbghelp then parses `victory.exe`'s Rust DWARF (`SymInitializeW` -> `SymLoadModuleExW` ->
`dwarf2_parse_subroutine_type`, dbghelp+0x2201e / +0x1d549) and reads garbage: a second crash that
hides the first exception and misleads the classification. Evidence: 62 `stderr.log` files with an
unhandled `dbghelp.dll+0x...` fault (report `reports/2026-11-28-crash-tory-sha.md`).

*Scope.* Shared layer (no platform switch in `HostPolicy`; Windows' dbghelp does not crash, but a
minidump is of no use to us anywhere and the title still writes its own `crash.log`). `crash.rs`
(`RtlCaptureStackBackTrace`) and the timeline report (loads the system dbghelp itself) are unaffected.

*Remove when* Wine's dbghelp parses Rust DWARF without crashing (retest after a Wine bump).

### H3 — how often the title calls each HLE function: `VICTORY_MAC_HLE_COUNTS=<file>` (`victory-hle/src/mac_counts.rs`, 3 hooks)

*What.* Off by default. `VICTORY_MAC_HLE_COUNTS=<file>` (empty, `0`, `off`, `false`: off) puts a 32-byte x86-64 stub in front of
every function of ours the title is given (`mov r11,<counter>; lock inc [r11]; mov r11,<function>; jmp r11`: `r11` is volatile and
carries no argument, the stub ends in a jump, so it fits every signature and leaves no frame) and writes the counts per
`module` (the `HleModule` name) and function as a text table to the file: every 5 s while they change
(`VICTORY_MAC_HLE_COUNTS_SECS=<n>`, at least 1), and when the process ends (the title's `exit`, `abort` and `TerminateProcess` on itself call
`mac_counts::flush`, and an `atexit` function writes it for any other CRT exit). The file is replaced atomically (written beside it,
renamed). First line `# victory-hle-counts v1`, then `# pid=.. uptime_s=.. reason=periodic|exit|abort|terminate bound=N called=N total=N`,
then `module<TAB>function<TAB>calls` by calls, most first; bound functions never called are there with 0. Off: nothing is
wrapped, the registry returns the function's own address, the cost is one `OnceLock` read per bound import.

*Hooks in upstream files.* `lib.rs` (`pub mod mac_counts;`), `registry.rs` (`lookup` goes through `export_address`, which calls
`mac_counts::address_for`), `modules/kernel.rs` (the `GetProcAddress` result for our pseudo modules goes through the same
function; `flush` in `exit`, `abort`, `TerminateProcess` on itself). Upstream semantics are unchanged.

*Scope.* Only functions of ours (`Provenance::Hle`, and `GetProcAddress` on one of our modules); an import the host answers goes
straight to Wine. x86-64 Windows only: on any other host `address_for` hands out the function itself and the table shows 0s (a
warning says so). A function's address seen by the title is the stub's, so code that compares an imported address with `&function`
would see them differ (nothing of ours does).

*Tests* (`mac_counts::tests`, 11 under Wine, 9 natively): the stub's bytes; one counter and one stub per function bound twice;
exact counts from 8 threads (also through a real stub: 160,000 calls, no loss); arguments (4 in registers, 3 on the stack, a float)
and the result pass through a stub; the file is replaced whole and leaves no temporary file; the table is written when a child
process leaves through the CRT `exit` and lists what the registry bound in it; a locked table is skipped on the exit path instead of
hanging it. `tools/mac/wine-tests.sh -p victory-hle --lib mac_counts`.

*Use.* A run through `tools/mac/run.sh` with `VICTORY_MAC_HLE_COUNTS=<path>` in the environment: the table is what the matrix rows
`mac.hle.*` need for `freq` (rows with no entry yet: `net`, `media_bink`, `apu_acphal`, `xgameruntime_family`, `xasync`,
`lifecycle`, `rpcrt4`, `audio_xaudio2`, `kernel_overrides`, `fidelityfx_xcrd` in the plan of 2026-11-28). *No game run has been made
with it yet.* *Remove when* the frame-time work no longer needs call counts; nothing else depends on it.

### T1 — `victory-shader-dylib/tests/translate.rs`: corpus folders from the environment

The test had two build-machine paths of the Windows team written in (`...\build\livetr-corpus`,
`...\build\rt1-work`), which do not belong in a public history. Changed in place (about 6 lines,
Windows file, only the new `mac:` commit touches it; the Windows line's history is as it was):
`VICTORY_LIVE_CORPUS` now holds one or several folders (separated like `PATH`: `;` on Windows), and
when it is unset the container test returns with a "skipped" message instead of asserting. To get
the old behaviour on the Windows machine, set `VICTORY_LIVE_CORPUS` to the two folders. Remove this
patch if the Windows line makes the same change.

### M6 — `tools/mac/push-check.sh`, `docs/mac/PUSH.md`

The check before a push to the public `fork`: policy, credential scan, privacy scan (internal
addresses, home-directory and drive paths, `*.local` e-mail addresses, the author of `mac:`
commits, a private word list kept outside the repository), binaries and game files. Also
`.gitignore` entries for `tools/mac/privacy.local.txt`. Usage in `docs/mac/PUSH.md`. Pure Mac
tooling, nothing to recycle; delete it if pushes to the fork stop.

### M8 — heavy tasks: `tools/mac/heavy.sh`, `heavy-guard.sh` (a child of `run.sh`), `check.sh` and the other heavy scripts through it

A machine-wide budget of job units (the number of cores; 1 while a game holds the game lock), nice 19, `SIGSTOP`/`SIGCONT` of the
later jobs when a game starts, slots in `research/.heavy/`, nested calls free (`VICTORY_HEAVY_SLOT`). `heavy-guard.sh` renices (and, at
load > 30, stops) heavy processes outside a slot, only while the game lock is held; it is started by `run.sh` (`heavy-guard.sh run --parent <run.sh pid>`, pid in
the lock directory) once `run.sh` holds the lock and is ended by `run.sh`'s cleanup before the lock is let go, with SIGCONT of everything it stopped; it
ends by itself (SIGCONT first) when its parent is gone (`run.sh` killed with SIGKILL, looked at every second) or the lock is not held any more. No launchd agent
(macOS keeps a launchd job's bash out of `~/Documents`): `install`/`uninstall` are gone (they report 已改由 run.sh 托管). `check.sh`, `wine-tests.sh`, `rosetta-probe*.sh`,
`census-*.sh` start with an `exec heavy.sh`; `rosetta-probe-wine.sh isa` keeps to `VICTORY_HEAVY_JOBS` probes; `push-check.sh` and the
no-`check.sh` fallback of `sync-windows.sh` wrap their `cargo`. The old "nice 15 while the lock is held" block of `check.sh` is gone.
Tests `test-heavy.sh`, `test-heavy-guard.sh`, and the guard-as-child cases in `test-run-lock.sh`; rules in `docs/mac/RUN.md` "Heavy tasks". Pure Mac tooling, nothing to recycle.

### M9 — `run.sh --purpose "<map column>: <node id>,..."` is required (`tools/mac/purpose.sh`)

Every run that starts the game says which column of the translation map (`freq`, `callstats`, `new-nodes`, `corpus`, `perf-hotspot`) it
fills and for which nodes; without a fitting one `run.sh` refuses with exit status 7, before it touches anything (`--print-env` and `--help`
need none). The text is the first line of `<out>/RUN.txt` and is kept in the game lock (`game-lock.sh status`, also for the queue).
`play.sh`, `drop-ab.sh`, `rt-builder-accept.sh run` pass it on. Tests: `test-run-until.sh` (refusals: none, bad column, no node id, ...),
`test-run-lock.sh`. A rule of the integration lead, not a technical need: delete it when runs for the picture are allowed again.

### M10 — frame times marked against the load: `tools/mac/frametime-summary.sh`, the end of `run.sh`

Only the end of `run.sh` (the summary) changed. The frames of `frametime.log` that fall into a 10-second stretch of `load.log` with a load
average above 30 are invalid; `summary.txt` gets the median and p90 of the valid frames, the invalid share, and `数据无效：load 超过 30` as its
first line when more than 20% are invalid. `frametime-annot.csv` has the marking per frame. `VICTORY_LOAD_LOG_INTERVAL` (test knob, default
10 s) is the load.log interval. Test: `tools/mac/test-frametime-summary.sh` (made-up logs, and `run.sh` with a fake Wine and a fake `uptime`).
Pure Mac tooling.

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
| data / cache directory | `data/livetr-perf-<backend>` | `data/livetr-fidelity-<backend>`; each cloned from the read-only seed `data/livetr` on first use (`--fresh-data` clones again); the game directory is never written |

The default stays **perf** until the 3D-missing question is settled and the owner says so; then the switch is one line,
`profile="${VICTORY_MAC_PROFILE:-perf}"` -> `fidelity` in `run.sh`, plus (separately, as a loader change) making `VICTORY_OUTPUT=0` the
code's default in `resolution.rs` `default_output_on`, so that a start without `run.sh` agrees. Perf stays as the explicit profile for
debugging and speed work, marked `non-fidelity` in the log, and no fidelity result may cite a perf run.

### I2 — record and capture by world frame: `w<N>` (`victory-gpu/src/mac/frames.rs`, 3 hooks)

`VICTORY_RECORD_AT=w<N>` and `VICTORY_CAPTURE_FRAMES=...,w<N>,...` name the **N-th world frame after world
frame 1** (I1's clock): `w0` is world frame 1, `w300` is world frame 301, which is present `first + 300` where
`first` is the present that latched world frame 1 (788 in one run, 1209 in the next: a present number such as `f300`
names a different moment each time, `w300` the same one). `f<N>` and bare present numbers work as before, and
`VICTORY_CAPTURE_FRAMES` may mix both (`64,w12`). Off by default: nothing changes unless a `w<N>` is given.

*How.* The Mac layer owns the parsing and the question: `frames::parse_world`, `parse_list`, `listed` (is this present
one of the targets, given `world::first_present()`) and `world::reached(n)` ("has world frame 1 + n been reached",
asked of the clock at every present). Upstream edits are one line each, marked `mac (I2)`: `replay/record.rs`
(`At::World`, the `w<N>` branch of `parse_at`, and `log_latch()` so the log says when world frame 1 latched),
`replay/snap.rs` (the `At::World(n) => world::reached(n)` arm beside `At::Frame`), and `capture.rs` (`listed()` takes the
list through `frames::listed`). Tests: `mac/frames.rs` (native) and `record.rs`'s `parse_at` test.

*Log switch (M4).* `VICTORY_LOG_WORLD=1` (set by `run.sh --until-world`, and by `--until-capture` with a `w<N>` frame) makes
`world::on_present` log `world frame <n> present=<p>` for every world frame from 2 on, and the existing `world frame 1 ... present=<p>` line
without a recorder; `tools/mac/until-watch.sh` stops the run on them. Off (no new line) when unset or `0`.

*Notes.* A `w<N>` capture lands the same few frames after the trigger as `f<N>` does (the recorder arms, then takes
the next whole frame). `VICTORY_CAPTURE_FRAMES=w0` cannot trace the world's first frame's draws ahead of time (it is
only known to be world frame 1 once presented); use `w1` or later. The world starts only once a frame has 500 title
draws (`VICTORY_WORLD_DRAWS`); with `w<N>` and no world ever reached, nothing triggers.

*Fidelity.* None affected: it only chooses which frame is recorded or saved. *Remove when* the Windows line's
recorder names world frames itself (then drop `frames.rs` and the three hooks; `world.rs` stays for I1).

### P4 — the title window's client area is the size the title asks for (`victory-hle/src/modules/lifecycle.rs`, shared layer)

*What.* A console title passes its picture size (3840x2160, or 1280x720 with L3) to `CreateWindowEx`, which on a PC
is the size of the OUTER frame. Wine took the caption and borders off it: 1280x720 became a 1272x686 point client
area (1.854:1), into which the 16:9 swap chain was stretched (4.3% vertical squash; Metal back, `present-size-20261127`).
`fit()` now adds the frame (`AdjustWindowRectEx` with the window's own style, ex style and menu flag) to the asked
size, shrinks the CLIENT size to what the work area (and `VICTORY_WINDOW_MAX`, which stays a limit of the outer frame)
leaves after the frame, keeping its proportions, and creates the window with the outer size. Child windows and
`CW_USEDEFAULT` are untouched. Log: `title window created asked=... outer=... client=...` for every top-level
window (the client size must equal the asked one unless it was fitted to the screen), and the existing
`title window fitted to the screen` now also names the frame.

*Shared layer, no Mac condition.* The host's own window (`victory-gpu` `host.rs`) already does this with
`AdjustWindowRect`; the title window did not, so on Windows the same code also made a client area that is smaller than
the swap chain by the frame. Not a Wine quirk.

*Test.* `lifecycle::tests::the_frame_is_added_to_the_client_size_the_title_asks_for`,
`a_shrunk_window_keeps_a_16_by_9_client_area` (rules), `a_framed_title_window_has_the_client_area_the_title_asked_for`
(a real framed window, client area 640x360); `lifecycle` is Windows-only, so run them with
`tools/mac/wine-tests.sh -p victory-hle lifecycle`.
*Remove when.* Never needed again: it is the right behaviour on every host.

### P5 — fidelity: the title outputs 4K, the host shows it at most 1440p (`tools/mac/run.sh`, no title patch)

*Finding (2026-11-28, Metal back, first screen's hint bar at present 320, no keys).* With the title unpatched (`VICTORY_OUTPUT=0`) the
presented picture is 3840x2160 whatever `settings.xml` `resolutionFullscreen` (2560x1440, 1920x1080, 1280x720) and whatever display mode the HLE
reports (`VICTORY_DISPLAY_MODE` 2560x1440, 1920x1080, unset); the title asks for a 3840x2160 window before it asks for the display mode. Its UI is laid
out in 4K pixel units: the hint bar is 604x54 px in every run, anchored to the output's right and bottom edge (81 and 60 px), so 15.7% of the width at 4K
and 47% (3.0x too big) with the L3 patch's 1280x720 output. Settings' `uiResolution` (0x0 in the seed) does not change it either; `settings.xml` was
not read after it was written (atime equal to mtime), a weak sign. Logs and pictures: the `ui-scale-2026-11-28` log folder of the integration lead.

*What.* The fidelity profile no longer pretends the title is asked for 1440p: it outputs 4K, as it does on a Series X, where the system scales to
the television. `run.sh` fidelity sets `VICTORY_PRESENT_MAX=2560x1440` (P1) and `VICTORY_WINDOW_MAX=2560x1440` unless set (or `--present-max`), so the
host shows the picture at most 2560x1440; the title's render targets and its UI are the 4K ones. `--print-env`, `run-env.txt` say so. The
`settings.xml` write stays (the title's menus may read it), documented as not deciding the output.

*Open.* Where the title takes 3840x2160 from is not found (not the display mode, probably not `settings.xml`); the perf profile's 1280x720 output has a UI that
is 3x too big (a UI scale that L3 does not patch). Both are title facts to chase in the disassembly before any patch.
*Remove when.* The title's own output size is found and can be set without a patch.

### S2 — mesh shader outputs numbered by alignment (`victory-shader/src/emit/mesh_order.rs`, 6 lines in `emit/mod.rs`)

*What.* The translator numbers a mesh shader's vertex and primitive output elements as the console's signature
has them. `emit_program` now sorts the two lists (`mesh_order::vertex`, `mesh_order::primitive`: position first,
then at each step the element that needs the fewest padding bytes in Metal's struct, the bigger one first on a tie)
before `lower`, which takes an element's id from its place in the list, so the metadata and every
`storeVertexOutput` / `storePrimitiveOutput` follow. Only mesh stages are touched (`StageIo::Ms`); D3D12 links
stages by semantic, so the order is of no meaning to a pixel shader. Upstream edits are the `mod mesh_order;` line and
the `if` block in `emit_program`, each marked `mac S2`. The translation fingerprint changes with the source hash, so
cached translations are made again. `back` keeps its run-time repack (`mesh_fit::repack_outputs`) as a fallback.

*Why not needed on Windows.* AMD and D3D12 have no per-mesh-vertex-struct size limit that counts padding: the DXIL
limit (32 KB) counts 4 bytes a component. Metal counts `round_up(NV, 4) x vertex_stride + NP x primitive_stride`
against 32768 bytes, with every struct member aligned to its own size and the members in element-id order (measured
on an Apple M5 Max, macOS 27.2.1, SDK 27.2, MSC 4.0.3, 2026-11-27; `docs/metal/mesh32k.md` on the metal line). A
`float` between two `float3`s costs 4 bytes on every vertex: 520 of the 8,765 translated mesh shaders of the title were
refused for the total size. S1 adds per-primitive outputs, whose stride counts the same way: they are ordered here too.

*Remove when.* The Windows line numbers mesh outputs by alignment itself (merge conflict at the `mac S2` lines), or
Metal stops counting padding.

*Evidence (2026-11-27/28).* Unit tests `emit::mesh_order::tests` (4: the alternating float/float3 case 276 -> 192
bytes, nothing lost or reordered when already packed, clip distances counted, primitive outputs 40 -> 32 bytes).
Census on the 8,765 translated mesh shaders, the translator before and after S2 and each built into a Metal 4 mesh
pipeline with MSC and the default pixel shader (`victory-census translate --hashes` then `convert --kind ms`):
refused for the total size **520 -> 143**, builds 8,245 -> 8,622, 0 shaders that built before stop building. The
143 left are real content (4 to 36 bytes a vertex over); dropping what the title's real pixel shaders do not read
saves 0 bytes for them. Frame-capture replay (`tools/metal/replay.sh`, `rec-a-f300`) with S2 on the metal line's back:
`replay.png` byte-identical to the replay without S2 (md5 458e2f66...).

### S3 — the shared translator: wave64 compute programs on 32-lane host waves, the shadow-mask ray programs (`victory-shader`, 5 hooks)

Census of the 31 compute programs the translator refused (2026-11-27, host wave 32): 26 "no wave32 lowering for this wave64 program",
5 "a descriptor read only as data". After S3 as first committed, 24 translate (20 wave64, 4 of the 5 shadow-mask programs); 7 stay refused (below).

**Update 2026-11-28 (`mac/wave-wide-off`): `VICTORY_WAVE_WIDE` is back to off by default; only the address table (item 1) stays on.** See "Wave-wide default off" below. Items 2-4 and the "24 translate" numbers describe the code with `VICTORY_WAVE_WIDE=1`; by default 4 translate (the shadow-mask programs) and the 20 wave64 programs are refused again.

*What (all in `crates/victory-shader/src/emit/`, every changed line marked `mac S3`).*
1. `mod.rs` `viewaddr_table()`: `VICTORY_VIEWADDR_TABLE` is on unless `=0` (was off). The shadow-mask ray programs
   `CS_TraceShadowRays_{Full,Half,Quarter,Eighth}` (`07c93bfb77fecebb`, `002e93a6cc2aa355`, `0e64f14fae060d04`, `96ccb14ef981fd85`)
   pick a buffer view from the heap at a computed index and only walk the memory behind it: the address comes from the host's
   address table (`t251`/`t250`, which the host keeps always). They were refused as "a descriptor read only as data"; the code was
   upstream's, switched off. Only programs that were refused change.
2. `mod.rs` `emit()`: `VICTORY_WAVE_WIDE` was on unless `=0` (upstream: on for two digests, off otherwise); **now reverted to upstream's rule** (`wide_branches_on`: `=1` turns it on, the two scene kernels keep their default). It was meant to act only after every other form
   failed, but the 2026-11-28 census shows it also changes 36 containers that translated without it (a different, larger DXIL; see below).
   Wave-wide branches give 20 of the 26 wave64 programs (LDS read and written inside a lane branch, a 64-bit ballot, `v_readlane`/`v_mbcnt`
   across the halves), groups of 64 threads.
3. `mod.rs` `widen()` and `lower.rs` `note_barrier`/`barrier_in_wide`/`lane_regions_of`: groups of several console waves are tried too;
   a group barrier (the title's, or the halves' meeting) lying in a wave-wide branch's region refuses the result as before: the console
   waves of a group can decide apart, and the barrier would wait for waves that never come. No hang is ever produced.
4. `lower.rs` first-lane exchange (`v_readfirstlane_b32`, every lane running): both of the exchange's LDS loads stay (the
   upper half's is ORed in under a mask of zero). With the one load alone the optimised module read zeros on D3DMetal (found by the
   S3 hardware test; with the second load kept, or `VICTORY_SHADER_OPT=31`, it was right).
5. Tests: `s3_tests.rs`.

*Why not needed on Windows.* The Windows line has hosts whose waves are 64 (AMD) and takes these programs as they are; the 32-lane
forms are NVIDIA/Apple only. `VICTORY_WAVE_WIDE` was kept off after runs 121-124 lost an NVIDIA card with wide-lowered
`ptfx_CS_lightAtlas_*` (cause unknown; not Metal).

*Not done (the 7).* `DefragHeap_ComputeRelocations` x2 (`22338086ea4c5364`, `02384a60fda6f2d0`, group 128), two 512-thread programs
(`1d434307d981352a`, `3b02415f67c6b0d8`) and `CollectTriangles` x2 (`6832a9da15c2e584`, `6b82bc7d22016cb5`, 1024): their wide branches
hold group barriers, and a branch taken by the group as a whole (a flag in LDS ORed over all waves, with the region's scalar results selected by
the wave's own decision) is not built. `CS_TraceShadowRays_FullAdaptive` (`dbdbf36298727135`): "a lane mask holding lanes elsewhere read
as data" at 0xae80 (a mask built as `s_or` of two masks, one a join of a descriptor dword and a mask from a stack of lane masks/numbers).
`CS_TransmissionVolumetrics*` x5 translate but Metal Shader Converter 4.0.3 aborts on their `double` ("Unhandled LLVM type for
getMinMaxConstants"): 19 of the 24 build a pipeline.

*Fidelity.* Exact: wide branches, the exchange, the barriers, the table addresses. Not exact: none of the 24 uses a local-wave
(per half) form, which is the only approximate rung of the ladder. `VICTORY_FIRSTLANE_OWN` stays off (it is exact only when the value
is the same across the console wave; the three BVH-builder first-lane programs `dc78c5de69c7c155`, `322ed4ad832bdea6`,
`d441b097704ab616` translate without it, and with it the DXIL is byte-identical).

*Wave-wide default off (2026-11-28, branch `mac/wave-wide-off`).* Why: with S3's default (wide on), the wide branch puts a
group barrier in a divergent path. On an Apple GPU `ptfx_CS_lightAtlas_Sprite` (compute `8d74b76fb8c0cb8d`, container
`2561af9577c47289`, a single-thread group) hangs for about 100 s per dispatch; in the prologue the long frames start around
w54-70. With `VICTORY_WAVE_WIDE=0` the prologue ran to w200 and w300 with no long frame (`research/game-runs/20261128-fidperf/stall-g`,
`stall-h`). The upstream comment (runs 121-124 lost an NVIDIA card) is the same cause. What changed: only the `wide_on` line in
`emit()` (`mod.rs`, marked `mac S3`; helper `wide_branches_on`); `VICTORY_VIEWADDR_TABLE` stays on (the 4 shadow-mask programs
need it). Tests: `s3_tests.rs` (`the_wave_wide_branch_is_off_by_default`; the wide case asks for `VICTORY_WAVE_WIDE=1` explicitly).
Effect (census `victory-census translate`, whole package, host wave 32, 25 985 containers; data
`research/census-widefix-20261128/{off,wide1}`): default 25 760 translate (225 refused); `VICTORY_WAVE_WIDE=1` 25 780 (205 refused). So the
20 wave64 programs are refused again ("no wave32 lowering for this wave64 program": their dispatches are dropped, as before S3). The
census also found that wide on changes the DXIL of 36 containers that translated without it (e.g. `2561af9577c47289`
101 140 B -> 146 112 B, the non-wide and wide forms; `ptfx_CS_lightAtlas_Trail`, `CS_GenerateShadowMasks*`, `CS_DeferredFallback*`,
`CS_IrradianceCache_*`, `CS_Gather*`): the earlier claim "programs that translated before are byte-identical" held for
which programs translate, not for their code. By default those are back to the pre-S3 form.
Wide stays available with `VICTORY_WAVE_WIDE=1` (the live-file switch) for experiments, never in a run that matters.
*Root fix (2026-11-28, branch `mac/wave-wide-fix`, node S-I-016, S3 wide form: shader chain, wave64 -> 32 divergent barrier).*
The earlier reading "a group barrier in a divergent path hangs the group" was not what happens. `ptfx_CS_lightAtlas_Sprite`
(`2561af9577c47289`; `numthreads(64,1,1)`: ONE console wave64 = two host wave32s of one group; LDS 3 dwords: [2] a wave counter,
[0],[1] the halves' exchange slots; 108 `dx.op.barrier` in 13 blocks, each half-to-half exchange = store slot / barrier / load both /
barrier) has, in its GFX10 code, a lane skip at 0x377c (`s_and_saveexec_b64` + `s_cbranch_execz` to 0x6150) around the loop
0x3898..0x612c, a loop that ends on `v_cmp_eq_u32 vcc, s65, v66` + `s_cbranch_vccz` (and an inner waterfall over a mask's bits).
`ir::without_skips` (the "scalars followed" form the wide form is built on) left that skip out, so a group in which no lane is in the
skip (a tile with nothing to light) ran the loop with EXEC empty: the compare finds no lane and the loop turns until the lowering's
bound (4096 turns, itself inside the outer 4096), with ~20 group barriers per turn: ~100 s of GPU time per dispatch (log:
`gpuprof: a scope took 104667-110807 ms: execute-indirect cs 8d74b76fb8c0cb8d`). Not a mismatch of barriers between the two host waves:
the static CFG walk of the DXIL found no branch around a barrier whose condition differs between the halves (the barrier-holding branches decide on values both halves exchange; `research/widefix-20261128/ROOTCAUSE-NOTES.md`).
Fix, all in `victory-shader` (marked `mac S3`): (1) `ir::without_skips`'s stall check (`stalls`, the Windows line's `VICTORY_SKIP_STALL`,
cherry-picked as 9da1676e) now also knows a compare into VCC/SGPRs under an empty EXEC (no lane), `s_and` of masks with one empty,
`s_cbranch_vccz/vccnz`, and calls a region stalling when every way on was followed and none left it (before: only a decided chain
that came back); the skip stays and `widen()` takes it wave-wide: the whole group decides (an LDS flag per host wave, barrier, OR over
both). (2) `lower::branches_around`: a one-block loop closes on its own branch, which counts as outside its region (a block post-dominates
itself); that branch is now in the region, so the cross-lane code of the loop is taken by the whole group, not per lane exit (found by the
hardware test). Evidence: unit `a_skip_around_a_loop_that_never_ends_with_no_lane_left_is_kept` (fails on 035b3728's `ir.rs`),
`the_loop_behind_a_skip_translates_wide`; hardware (Wine, D3DMetal; `tools/mac/wine-tests.sh -p victory-shader s3_tests`)
`two_host_waves_with_different_conditions_take_the_loop_behind_the_skip_together` (a cut-down of the loop, 8 conditions: no lane in the skip,
a few lanes of the lower half, the lower half, 33, 40 and 64 lanes; 60 s limit; fails without the fix, passes with it).
Census `victory-census translate`, whole package, 25 985 containers, host wave 32: default 25 760 translated (225 refused), the same
set and every DXIL size as before the fix; `VICTORY_WAVE_WIDE=1` 25 780 (205 refused), the same set as before the fix, of which 12 DXILs change
(the 7 programs of the 36 whose wide form the kept skip changes: `ptfx_CS_lightAtlas_Sprite` 146 112 -> 146 592 B, `_Trail`, `CS_Gather*`,
`CS_RTConstantCost...`, `CS_TraceShadowRays_Full`, `CS_ProjDecalResolve_Vectorized`; and 5 of the 20 newly translated water programs).
Data `research/census-widefix-20261128/{fix1,fix-off}`. Prologue with `VICTORY_WAVE_WIDE=1`, new game, diagnostics exe `0121e7be`
(`research/widefix-20261128/run-w300-fixed2`): world frame 300 at 172 s, no frame over 5 s (the longest, 4.4 s, is the world-frame-1 load),
no `a scope took`, `2561af9577c47289` bound at present 1220. Before: 4 of 4 runs with ~100 s frames from w54-70 on. The default stays OFF
(the owner decides when it comes back).

*Remove when* the Windows line takes `VICTORY_WAVE_WIDE` on by itself and the address table is default there (then the two defaults
are upstream's).

*Risk.* An upstream comment records that the wide form lost an NVIDIA card, and on Apple GPUs it hangs single-thread groups (above): it is off by default here. With the root fix (above) `VICTORY_WAVE_WIDE=1` ran the prologue to w300 without a long frame; turning the default back on is the owner's call.

### S1 — `GetAttributeAtVertex` without `attributeAtVertex`: the other vertices as flat inputs (`victory-shader` `emit/{vertex_flat,lower,mod,wire}.rs`, `victory-shader-dylib/src/lib.rs`, `victory-gpu` `host.rs`, `pipeline.rs`, `pipeline/host.rs`, `shader_cache.rs`, `mac/attribute_at_vertex.rs`)

*What.* A new translator capability `attribute_at_vertex` (`victory_shader::emit::host_attribute_at_vertex()`, set with
`set_host_attribute_at_vertex`; default **true**, which is the Windows line: its output stays what it was). victory-gpu
sets it at host creation from `mac::attribute_at_vertex::host_has_it()`: **false** on the Metal chain and under Wine on
a Mac (D3DMetal), true on Windows; `VICTORY_ATTRIBUTE_AT_VERTEX=0|1` overrides it (and is the default for tools that
translate without a victory-gpu host: victory-census, the metal tests). It travels in `host_settings()` as bit 31 of the
last word, so the shader cache and the live translator library (`victory-shader-dylib`) key on it. With it false:
* a pixel program reads the three vertices of an attribute as flat inputs instead of `dx.op.attributeAtVertex`: vertex 0 is the
  attribute's own constant input (the provoking vertex), vertices 1 and 2 are added per-primitive `u32` elements
  `TEXCOORD 64+n` (four scalars each); the pixel `HostShader` lists them in the new field `vertex_reads`
  (`emit/vertex_flat.rs`, stored in the VSH1 bytes after the views);
* a mesh pipeline: victory-gpu translates the mesh shader again with the pixel shader's reads
  (`emit::translate_with`, `Shader::translated_with`, `shader_cache::host_shader_with`, hook in `HostPipeline::new`): each lane writes the
  read channels of its vertex into group memory, every exit of the program leaves through one epilogue block that
  meets (barrier) and lane n writes primitive n's vertices 1 and 2 as primitive outputs; the added outputs count against
  the 32 KiB mesh limit, group memory over 32 KiB refuses the translation;
  the epilogue reads every exchanged value before it writes any output, and a 16-bit output is exchanged as its raw half
  (converted after the read): Apple's GPU compiler crashes (MTLCompilerService SIGSEGV in AGX) on the interleaved form;
  **known issue** (numbers from the Metal backend owner, 2026-11-28): there are 4834 real (mesh, pixel) pairs: 2170 build,
  1261 are over 32 KiB in total, 1400 crash Apple's compiler (AGX; 1273 pairs after de-duplication, already in the
  `mesh-pair-denylist`), 3 exceed the varying limit (2170 + 1261 + 1400 + 3 = 4834). The earlier figure of 1774 crashes was an
  artefact of compiling 4 pairs at once; rerun one at a time it is 1400. Only 3 pairs crash without S1 too. See research/attr-vertex/agx-crash/;
* a vertex pipeline: victory-gpu puts a geometry shader (`emit::vertex_pipeline_gs`: triangles in, the same triangle out
  with vertices 1 and 2 as constant elements) in the slot of the rectangle list's stage; a rectangle list with such a pixel
  shader, or a host that refuses the stage, gets no pipeline.
Every hook line in an upstream file is marked `mac S1`. Tests: `emit/vertex_flat_tests.rs`, `emit/vertex_flat.rs`
(victory-shader 209 -> 219), `mac/attribute_at_vertex.rs`; GPU pixel tests, the corpus test and the census on the `attr` branch
(`metal/back/tests/gfx_attr_vertex.rs`, `metal/census/tests/attr_vertex_corpus.rs`, docs/metal/attribute-at-vertex.md).

*Why not needed on Windows.* The Windows host (D3D12 with dxil.dll, the GPU driver) takes `GetAttributeAtVertex`. Metal Shader Converter 4.0.3
and D3DMetal refuse it (error 7), so 4852 of the title's pixel programs, and with them most of its mesh pipelines (102 of the 103
refused pipelines of one run), built no pipeline on a Mac. Native Metal has `vertex_value` only on Apple10 (M5+), and MSC does not emit it.

*Remove when.* MSC translates `GetAttributeAtVertex` natively, or emits MSL's `vertex_value` and the lowest supported GPU has it: then
drop the capability (the default true is then the only path), `vertex_flat.rs`, the field `vertex_reads` and the hooks (`grep 'mac S1'`).

*Acceptance (2026-11-28).* Translator tests 219 passed in victory-shader (209 before), 78 in victory-gpu; GPU tests against a CPU
reference (mesh, provoking vertex, VS + GS + PS) pass. Census with the capability off over the 25985 containers: no program emits `attributeAtVertex`;
of the 4852 pixel programs (plus the one `atomicBinOp.i64`) 4832 build through MSC and a Metal pipeline, 20 fail on Metal's 124 varying
scalars (126-127 with the added ones), 1 is the i64 atomic: **pixel programs refused for attributeAtVertex 4852 -> 0**; the 21 that remain fail for other reasons (20 over the varying limit, 1 i64 atomic). Mesh programs newly over 32 KiB by
the added outputs (no pixel-mesh pairing exists: assuming D scalars read, 128 primitives, 8245 under the limit now): D = 4: 324, D = 10
(median): 853, D = 22 (p90): 2362, D = 38 (max): 4504. Replays (`tools/metal/replay.sh`): f300 byte-identical picture to the earlier build (blake3 `713c6b13b79d2de4`),
world w1 and w100 identical to the recording, w300 differs from the previous build by as much as two runs of the same build differ (about 11-15 thousand pixels over 8/255 of 921600);
the replays log nothing about pipelines, so that the aav pipelines of those frames draw was not observed directly.

### S4 — geometry shaders on the Metal chain: clip distances of rectangle lists, and the title's own geometry shaders (`victory-shader` `gs.rs`, `io.rs`, `emit/{rect,lower,mod}.rs`, `victory-gpu` `pipeline.rs`, `pipeline/host.rs`, `shader_cache.rs`)

*What.* The translator's rectangle-list geometry stage (`emit::rect_list_gs`) used to be refused for the 29 vertex programs of the title
that write `SV_ClipDistance` (of 2192; matrix S-M-003): Metal Shader Converter 4.0.3 cannot make the geometry emulation's mesh function
of a pipeline whose geometry stage *writes* a clip distance ("Failed to materializeAll"; a stage that reads one but writes none is as bad,
a plain interpolant is fine; checked 2026-11-28, `metal/back/tests/gfx_clip.rs`). Now, when the host is the Metal chain
(`host_attribute_at_vertex()` false: the same marker S1 uses) and a rectangle list's vertex program writes a clip distance:
* the vertex program is translated again with its clip distances as a plain output named `VICTORYCLIP`
  (`emit::translate_variant(.., rect_clip = true)`, `emit::clip_as_varying`; `Shader::translated_as`, `shader_cache::host_shader_as`, a
  separate cache key; hook in `HostPipeline::new`, `rect_clip`);
* the rectangle stage reads those distances and writes none. It does the hardware's clipping itself: the rectangle is the parallelogram of its
  corner and the other two vertices in the unit square `(u, v)`, every output is affine in it, so each plane is a line in the square; the
  square is cut by each plane in turn (at most one vertex more each, up to four planes) and goes out as one triangle strip with every
  output evaluated at its vertices. A triangle that is no rectangle is cut the same way. Pixels drawn are those where the interpolated
  distance is not negative (D3D12's rule for `w > 0`, which a rectangle list has).
If the host refuses the stage the pipeline is remade without it (as before), and its vertex program then clips nothing.
Tests: `emit/rect.rs` `clip_distances_are_cut_by_the_stage_and_not_passed_on`; pixels against a CPU reference in `metal/back/tests/gfx_clip_rect.rs`.

*Why not needed on Windows.* D3D12's geometry stages write clip distances; the default capability leaves everything as it was.

*Remove when.* MSC makes a geometry emulation that writes `SV_ClipDistance` (rerun `gfx_clip.rs`'s refusal test: it then fails).

*What, part two: the title's own geometry shaders (2026-11-28; plan S4, the 195 `GS_VoxelizeGeneric_*` containers).* The container of a
geometry shader holds one program of the console's on-chip geometry stage (hardware stage `Gs`, wave64): one lane per input triangle, the
vertex shader's outputs in the ESGS ring (LDS) and its own vertices written to an LDS slice per lane, sent out by `s_sendmsg`. `translate`
refused it ("a container without a vertex, pixel, compute or mesh program") and `HostPipeline::new` refused every pipeline with a geometry
stage. Now:
* `victory-shader/src/gs.rs` takes the ring accesses apart. An evaluation of the integer arithmetic with offsets of its own for the
  vertices (`vertex_offset`, `vertex_pair`: what `v0`/`v1` hold) says which vertex and which dword every `ds_read*` reads (dword `4 row +
  channel` of the vertex shader's outputs); the lane's slice (`0xf0 * lane + base`, the slice size in bytes) says which vertex and dword
  of the output every `ds_write*` writes (`slice / emitted vertices` dwords a vertex, four per output register). `prepare` removes what only
  addressed them (including the loads of a ring-size table that trace to no root parameter, and the `s_bfm_b64 exec` setup: each host thread is one
  primitive that runs).
* `io.rs`: `Input::GsVertexPair`, `GsWaveInfo` for `v0`/`v1`/`s3`; `Io::gs` holds the analysis. `emit/lower.rs`: `gs_ring` turns a read into
  `loadInput(element, row, column, vertex)`, `gs_message` turns `GS_OP_EMIT` into `storeOutput` of the vertex's dwords and `emitStream`,
  `GS_OP_CUT` into `cutStream` (the shader declares triangles in, strips out, `max_vertices` = the emits); `v_mbcnt` is 0 in a geometry program (its lane is
  its primitive). Straight-line programs, 32-bit ring accesses, stream 0 only; anything else is refused with its offset.
* `HostPipeline::new` accepts vertex + geometry + pixel: the title's geometry shader is the last stage (visibility geometry, its own user data and descriptor
  tables bound like any stage's by `draw`); a pipeline with one and a rectangle topology, or whose pixel shader reads other vertices (S1), is
  refused; the host refusing it is a refusal of the pipeline (there is nothing to fall back to), logged as such in the drop statistics.
Tests: `gs::tests` (4, hand-assembled program), `metal/back/tests/gfx_gs_voxel.rs` (the 195 programs lowered, HLSL vertex and pixel shaders, six
triangles each facing one of the six directions, 1170 images against a CPU rasterisation of the triangle the program computes; the title's containers are
read from `VICTORY_GS_CONTAINERS`, `SKIPPED` without). Census (`victory-census translate`, then `convert --kind tgs --isolate`): translate Gs 0 of 195 -> 195 of 195;
Metal Shader Converter and Metal pipelines 195 of 195, 0 aborts.
*Not observed in a run:* no frame the replays and runs so far (prologue to w300, world drops) reaches a pipeline with a title geometry stage, so the drop statistics
see no difference (graphics-gs refusals 0 before and after); the host-side change is checked by the cross build and the GPU test only.
*Remove when.* never needed on Windows (D3D12 takes the geometry stage as it is); the host side stays with the S4 patch.

## Fixes taken from other lines (`git cherry-pick -x`, one `mac:` commit each)

Taken because they close a node of the translation map; the source SHA is in each commit message. Drop one when mac/livetr
takes a merge of its line.

### X1 — BVH builder pipelines queued at creation (`victory-gpu` `pipeline.rs`, `pipeline/make.rs`, `host.rs`; Windows ec648b34)

*What it is.* `builder_runs()` used to make every ray-tracing structure-builder pipeline on the recording thread at the first builder
dispatch (`make::now()`); on a cold cache that frame passed the title's 30 s GPU watchdog. Now they are queued at creation
(`make::prepare`), a dispatch looks at them without waiting (`make::state`) and skips while one is being made (frame line
"dispatches/frame skipped while its pipelines are being made"); only one that cannot be made turns the builder off.
`VICTORY_BVH_BUILDER_SYNC=1` restores the old behaviour. `BUILDER_MAKING` is `cfg(windows)` like its only other user (`host.rs`
is a `cfg(windows)` module), so no extra gate was needed for the native build. Matrix: `shared.rt.bvh_build`,
`mac.title.hang_watchdog` (unproven, 待运行验证: unit tests `pipeline::builder_verdict_tests` only).
*Retire when:* mac/livetr merges the Windows line that has it.

### X2 — divide-by-zero clamp in the upscaler (`victory-loader` `mac/resolution.rs`; origin 0cc44dc6, #615)

*What it is.* When the resolution patch shrinks the output, the upscaler's round-robin `div dword [rdi+0x10c]` (title
+0x28f03e8) can divide by zero and trap (0xc0000094; the "known divide by zero" of `mac.res.fsr`, PR #278/#362). `DIV_ZERO` is
replaced by a `call` to a 15-byte trampoline in `.text` padding (`DIV_TRAMP`, +0x5f40966) that clamps the divisor to 1. Both sites are
compared byte for byte (build 1.0.2.1 only), written together or not at all. *Mac change:* upstream applies it always; here only
when a resolution patch is asked (`VICTORY_OUTPUT` size, render scale, upscaler, pool check), so a fidelity run
(`VICTORY_OUTPUT=0`) still patches nothing and logs no `resolution:` line. Matrix: `mac.loader.resolution`, `mac.res.fsr`
(unproven, 待运行验证: unit test of the bytes only).
*Retire when:* the title no longer reaches the divide with the output patch (never expected), or mac/livetr merges #615.

### X3 — low-address guards with 2.75 GiB Wine slack, own views above 2 TiB (`victory-hle` `modules/xmem.rs`, `apps/victory/src/run.rs`; origin e68a2191, #398)

*What it is.* Under Wine the host's own memory (D3DMetal/Metal heaps, caches) and Wine's bottom-up heap/stack searches compete with
the title's 32-bit `XMemVirtualAlloc` requests for the space below 4 GiB. `xmem::low_guards::install()` (called at the start
of `run` when executing) reserves the free space below 4 GiB as `PAGE_NOACCESS` guards, leaving the lowest 2.75 GiB
(`WINE_SLACK`; 256 MB on native Windows) to the host, so about 1 GiB at the top stays for the title; a 32-bit request that
finds no room releases guards (smallest that fits first) and retries (`low_guards::retry`). `VICTORY_LOW_GUARDS=0` turns it off.
*Default (2026-11-28): off under Wine on macOS, on elsewhere.* On Windows the upstream default is unchanged (on). Under Wine on Darwin
(`victory_gpu::mac::rosetta::rosetta()`) `install()` does nothing unless `VICTORY_LOW_GUARDS=1` (`true`, `on`); the log then says
"space below 4 GiB left unguarded". Two reasons: (1) it changes the memory layout of every run (2.75 GiB left at the bottom, the top
~1 GiB reserved) and has not run once under Wine; (2) its reason for being, `mac.hle.low_address_commit` "3 of 8 runs", does not hold: G7 found
that the write of 0 at image+0x2215c35 is the title's own hang watchdog, not an allocation failure. Tests: `low_guards::tests::guards_are_off_by_default_under_wine_on_macos_only`
(and the switch test). *To turn it on by default:* run once with `VICTORY_LOW_GUARDS=1` to w300; the log shows no allocation failure
and no new crash; then flip `default_on` (xmem.rs) and update this entry and the matrix row. Our own views above 2 TiB (below) have their own switch.
*14:30 run (2026-11-28):* both switches are off by default under Wine on macOS; verify with `VICTORY_LOW_GUARDS=1 VICTORY_OWN_VIEW_HIGH=1`
to w300: no allocation failure and no new crash, then flip both defaults (`low_guards::default_on`, which `own_view_high_from` reuses) and update this entry and the matrix row.
Our own views of a section (`Section::window`, the per-write `View::map`) start at or above 2 TiB under Wine (`map_own_view`,
`MapViewOfFile3` with `LowestStartingAddress`, `OWN_VIEW_FLOOR`), so Wine's `try_map_free_area` does not step through the gigabytes
of host mappings under `virtual_mutex`. *Default (2026-11-28): off under Wine on macOS, on elsewhere* (switch `VICTORY_OWN_VIEW_HIGH`,
`1`/`true`/`on` enable, `0`/`false`/`off` disable; same parsing and platform default as `VICTORY_LOW_GUARDS`, `own_view_high` in xmem.rs):
off, `map_own_view` is a plain `MapViewOfFile` as before e68a2191; it has not run once under Wine and moves every view of every run.
On Windows `on_wine()` is false and the upstream behaviour is unchanged. Tests: `own_views_are_high_by_default_except_under_wine_on_macos`;
the floor test checks the 2 TiB floor only when the switch is on. Reason it was taken (it was "not ported, wait for a run that needs it"): `mac.hle.low_address_commit`,
the title got a NULL from an allocator and wrote 0 at image+0x2215c35, in **3 of 8** runs (run report 2026-11-27 crash 2).
*Mac port.* mac/livetr's xmem.rs never had the #1009 guards the commit widens (only the docs paragraph in `macos.md`), so the module
(`low_guards`, `FOUR_GIB`), its call in `run.rs` and the retry in `XMemVirtualAlloc` came along with it, in the form at e68a2191.
Our `Section` keeps its own design (`window` + commit bitmap, `VICTORY_SECTION_WINDOW=0`), not upstream's `whole()`: `map_own_view`
is used in `Section::window` and `View::map`; upstream's test of the shared whole view is not ported, its floor test is.

*Allocation-failure log (11-28, `victory-hle/src/modules/alloc_fail.rs`, called from `xmem.rs`).* Every point where memory is refused to the
title says so in one line, `xmem alloc failed: site=<where> n=<count at the site> size=0x... (N MiB) <detail>`, the first 32 per site and then each
power of two (the count `n` is exact): `XMemVirtualAlloc` (host refused after the low-address retry; kind, flags, range, error),
`XMemAlloc.hook(__XMemAllocationHooks__)` (**the title's own routine returned NULL**, with the attributes and the hook address),
`XMemAllocDefault.32bit` / `.layout` / `.heap`, `XMemAllocatePhysicalPages.args` / `.section`, `XMemMapPhysicalPages.unknown_page` / `.host`,
`XMemMapPhysicalPagesScatter`, `commit.physical_pages`. A run with none of these lines had no allocation failure at those points; so the
matrix row `mac.hle.low_address_commit` is cleared into `mac.title.hang_watchdog` (G7) unless a line shows up. Tests (native, the module is not
Windows-only): `alloc_fail::tests` (line format, the hook result with its attributes, per-site counts, logged-then-powers-of-two). The call
sites in `xmem.rs` are Windows-only and compile for windows-gnullvm; they are not run under Wine yet.
Matrix: `mac.hle.low_address_commit` (unproven, 待运行验证: unit tests only, none run under Wine).
*Retire when:* mac/livetr merges the line that has it.

## Not ported this round (decided with the Windows line as the reference)

Listed in `inventory.md` with source commits. In short: D3DMetal/Metal-specific GPU work
(`alias.rs` `mach_vm_remap` zero-copy buffers, indirect-clamp/DXC precompile, castable lists,
R32G32B32 views, 64-bit atomics, mesh limits, queue recovery, shader MD5 hash, wide indirect
dispatch) because `host.rs`/`draw.rs` diverge by tens of thousands of lines; **G3**: the DXC
helper (`VICTORY_HELPER_DXC`) is skipped in `victory.sh` until `victory-gpu/build.rs` has it;
DirectStorage per-worker handles (#400), publish rename
tolerance (this line's `publish.rs` already stages per writer), pipeline cache calls under Wine;
the generic upstream PRs in the Mac history (not Mac specific). Each is a candidate for a later
`mac:` commit once a run on the Mac shows the need.

Taken later (no longer "not ported"): the low-address guards and 2 TiB views (#398, with the #1009 guards it builds on), see X3.

### M7 — `tools/mac/play.sh`: hand play wrapper

*What.* A 20-line wrapper over `run.sh`: perf (since 2026-11-28; `--fidelity` is an option), no `VICTORY_KEYS`, no input replay, watchdog off (L5), 4 h limit; defaults `--exe` and
`--prefix` to the main checkout's when this worktree has no build. Docs: RUN.md "Playing by hand". `run.sh --worldsave` / `--new-game` and the refusal of a seed with a world save (2026-11-28): new game is the default, the world save is only on request. *Remove when.* run.sh grows a play preset.
