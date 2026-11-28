# Mac patches on `feat/live-translator`

Branch `mac/livetr` is `feat/live-translator` (the Windows line, base `150c16b4`) plus the patches
below. Every commit that is ours starts with `mac:`. The aim: new files and `cfg(...)`-gated
modules for the Mac, the fewest and shortest edits to files the Windows line owns, so that
`git merge win/feat-live-translator` stays cheap (`git log --oneline 150c16b4..mac/livetr` lists ours).

**GDKX (NDA) rule of the Mac patch layer.** The GDKX on this Mac is a local reference: use it for API semantics, structure layouts and compiler-generated oracles;
constants and structure definitions the implementation needs may be written into our code. Never commit a whole header or document of it as it is (take
semantics in our own words, note 「依据本机 GDKX 参照」). `push-check.sh` class `nda-gdkx` blocks the NDA header names and `gdkx`/`gxdk`/`private/` paths and the shared
pre-push hook enforces it (`docs/mac/RUN.md`, "GDKX (NDA)", patch M12).

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

*Verdict (2026-11-28, checked against the local GDKX reference, 10.1.26100.9661, and the disassembled call sites title+0x2477578 and title+0x24800d5).* Slots 0x1c8 and 0x218 are the Xbox extension's "deserialize a graphics / compute pipeline state": they load a pipeline that was serialized offline straight in. Returning `E_NOTIMPL` is a cache miss: the title falls back to slots 0x2b0 / 0x1f0 and builds the pipeline itself, so no draw is lost. That makes both slots **safe stubs** (matrix `C-V-001`: proven, disposition "安全桩"). 0x218 is implemented but stays off behind `VICTORY_MAC_PSO_X`; 0x1c8 is left out on purpose. The count 18,432 in the matrix is a lower bound (the log prints at powers of two), not the number of calls: the real count is between 20 and 30-odd thousand, which does not contradict the Windows line's 28,139. Still to do: log one total at the end of each run, and run the drop-ab B0/B1 A/B comparison to confirm.

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
Off unless set (one atomic load per texture). For ordinary CreateUnorderedAccessView calls naming a 3D view, compare the view range with the same resource's depth at MipSlice. Report cumulative full/strict_subrange/unknown at powers of two and on the first occurrence in each class. Null, unrecognised and invalid ranges are unknown. Explicit full depth is not a partial view. Nothing the title or the host sees changes.

*Why.* Map rows C-V-003 (detile) and C-V-005 (tile modes): the detiler is proven against addrlib for the modes without XOR
(`victory-formats/tests/addrlib_golden.rs`), and for sizes of the XOR modes; which modes the title really uses was never
counted (the capture `frame.bin` holds host-side descriptions only). The count decides whether XOR addressing (needs the
per-surface pipe-bank XOR) is worth writing.

*Why not needed on Windows.* The Windows line has the same detiler and the same question; this is a measurement, not a fix. *Remove when*
the census has been read and the answer is in the map row.

### G13 — shared GPU telemetry for 3D UAV ranges and device feature queries

`VICTORY_MAC_DROP_STATS=1` reports cumulative 3D UAV counts (full, strict subrange, unknown) and CheckFeatureSupport counts by feature ID and HRESULT. G11 layout stats can report the same view classification independently. No rendering behavior changes. Retire once equivalent upstream counters exist.

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

### T2 — the Wine run of the victory-hle and victory-loader tests: what failed there and what was changed (2026-11-28, branch `mac/wine-test-fixes`)

Four tests failed under Wine on Rosetta (`tools/mac/wine-tests.sh -p victory-hle -p victory-loader`); a fifth group of flakes showed once they were
repeated. Report: `GTA6-Mac集成/reports/2026-11-28-wine-fail.md`. One `mac:` commit each, `git log --oneline mac/livetr..mac/wine-test-fixes`.

| test | cause | change |
|---|---|---|
| `write_sites::changed_moves_and_vector_stores_do_what_they_did` | Rosetta hides AVX (cpuid leaf 1 ecx bit 28 and OSXSAVE 0, leaf 7 ebx 0) unless `ROSETTA_ADVERTISE_AVX=1` is set; the game runs set it, the test runs did not, so `is_x86_feature_detected!("avx2")` was false and the vector stub (correctly) was not made | `tools/macos/test-wine.sh` exports it (default 1); the test says so in its assert. Also turns on the avx2 paths of `victory-gpu` `torn.rs` and `shadow.rs` in Wine runs |
| `net::only_loopback_is_reachable` | environment: CrossOver Wine's ws2_32 lacks `WSAConnectByList` and `GetAddrInfoExA`, winhttp lacks `WinHttpGetProxySettingsEx`; the title imports none of them (0 call sites in the import census) | the test allows exactly those three on Wine, lists all missing names otherwise |
| `xgamesave::a_kept_blob_past_the_link_limit_is_copied` | environment: the temp folder is APFS, which has no 1024-link limit, so the premise (the 1025th link is refused) cannot be made | `link_or_copy` is a function; `a_link_that_fails_is_copied` tests the copy on every host; the NTFS test skips where the host has no limit |
| `xmem::a_title_thread_goes_on_using_the_rest_of_a_view` | a defect of Wine's view replacement (shown with no code of ours: `tools/mac/view-replace-probe.sh`), not of the HLE bookkeeping | the test keeps the fault-resume assert everywhere and counts lost data on Wine only; `xmem.rs` says (1st, 2nd, 4th.. time) when `unmap()` cut a view, the exposure count of `mac.hle.xmem_map_partial_view` (5 lines, `cut_a_view`) |
| `console_insn::a_clzero_loop_becomes_plain_stores` (11 of 15 suite runs) | the stub room was kept near the first site that asked; a site out of `jmp rel32` reach of it silently was not patched | a site out of reach gets a room of its own (`near()` call in `patch_clzero`, real code, also in the game) |
| `publish::concurrent_publishers...` (1 in 60), `crash::memory_at_names_what_an_access_hit` (1 in 25), `xgameui::dialogs_end_with_what_was_typed` (1 in 40), `files::long_title_paths_work_on_the_host` (1 in 37), `load_probe::loads_binds_and_calls_probe` (always) | a rename refused for the moment; a freed address reserved again by a neighbouring test; `XAsyncGetStatus(wait)` between `complete_call` removing the call and publishing the status (a race on Windows too); a sharing violation after fclose (Wine); cargo spawned from Wine | retry of the rename (`publish.rs`, real code); tolerant test; wait for the status (`xasync.rs`, real code); retry in the test; the fixture DLL that `test-wine.sh` already built is used |

*Retire when:* the Windows line makes the same changes (the two `victory-loader` and one `xasync` code changes are not Mac specific and can go upstream).

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

### M11 — the stage's capability file `CAPS`: `run.sh` exports `VICTORY_METAL_MESH_COMPUTE_SUPPORTED=1` only for a stage that says `mesh_compute`

Only `run.sh` (after the stage is known), `test-run-until.sh` and `docs/mac/RUN.md` change. If `$stage/CAPS` has a whole line `mesh_compute` (lines
starting with `#` are comments, one capability per line) the title gets `VICTORY_METAL_MESH_COMPUTE_SUPPORTED=1`; otherwise nothing is exported and
a value set by hand is kept. Reason (S5): the VMCS part appended to a mesh container over 32 KB changes the container's hash, so 966 entries of an old
back's AGX crash list would no longer match; an old stage with a new exe must not append it. `--print-env` shows the variable and its source
(`# metal: caps:`). Metal-backend only; D3DMetal unaffected. Tests: `test-run-until.sh` (CAPS with `mesh_compute`, with other lines only, without a
file, `--print-env`, and a run with a fake Wine). Pure Mac tooling; drop it when every stage carries the part's support (then S5 defaults on).


### M12 — NDA guard: `push-check.sh` class `nda-gdkx`, `push-check-nda.py`, `install-hooks.sh`, `test-push-check-nda.sh`

Path rules only: NDA SDK header names (`d3d12_x*.h`, `xg*.h`, `XGame*.h`, `*_xs.h`, `pix3_xbox.h`, `dstorage_xs.h`, ...), `gdkx`/`gxdk` directories, `private/` segments;
every hit blocks (no REVIEW, no waiver). `install-hooks.sh` installs the shared pre-push hook (managed block, idempotent, existing hook kept) and the engine
copy under the git common directory. Rules in `docs/mac/RUN.md` "GDKX (NDA)". Pure Mac tooling, nothing to recycle.

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
| output | native 1440p by default since X13 (log `non-fidelity: output patched to 2560x1440 (native 1440p, in the image)`); the 1280x720 L3 patch (`VICTORY_OUTPUT=720`, log `non-fidelity: output patched to 1280x720`) is the alternative | the title's own, from `settings.xml` set to 2560x1440 in the profile's own data directory copy; `VICTORY_OUTPUT=0`, no resolution patch |
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
*Update (X13, 2026-11-28 evening).* Where the title takes 3840x2160 from: the SDR display mode record at +0x0615c100 (5th entry of the mode table at +0x0615c0c0; the byte test of X13 reads it on the title image).
The Windows line's native 1440p chain patches it; the perf profile now does by default, the fidelity profile does not (it stays the title's own 4K, no patch). Whether the UI follows the record: matrix `mac.res.ui_scale`, to be measured.
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
Semantics of the 36 changed containers (local branch `mac/widefix-equiv`, not for push: `equiv_tests.rs` + `tools/mac/equiv-plan.py`, on the Mac GPU,
40 fixed-seed input sets each, wide form against the form the translator picks without it): 15 of the 36 have a plan the harness can bind
(the others index views from memory, or use 1D/3D/array textures the executor cannot bind). Of the 56 containers (36 changed + 20 newly translated), 36
have a form without wide and 20 have none; 51 of the 56 wide forms pass Metal Shader Converter and Metal on the executor (`equiv_compile`), and 5 of the 20 new ones
are refused for double precision (MSC 4.0.3 cannot convert it: the back must keep stopping them before MSC).
Of the 15: 6 identical in every run (2 of them never store: no coverage), 4 identical except 1 ulp in a denormal (8 dwords, `0x8f7` vs `0x8f6`),
2 not run (no store happens), 3 differ (`CS_ClusterLights`-class `55d2bce91ec9abcc` and its twin `d3e5b737ea7cc150`: wide has every bit the other has plus the other
half-wave's; `8dd8d739047fe557`: a whole-wave prefix sum + atomic allocation, the other form's two halves each scan alone and
overlap, 16 342-16 406 non-zero dwords against 16 374-16 438). Those three are programs for which the other form is the "APPROXIMATE, any mask" local-waves
form, so the difference is the other form's error, not the wide form's; there is no exact reference form for them (that is why the wide form exists).
Not proven: no GFX10 interpreter exists here to give the console's own result. Results: `research/widefix-20261128/equiv/containers/*.result`.

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
*Default on everywhere since 2026-11-28 (4 runs: endings independent of X3).* On: w1009 exit(0), w815 at 5149aec. Off: w705 at 5149aec, w1153 exit(0). All four had zero xmem allocation failures. Both outcomes occurred with either setting. `VICTORY_LOW_GUARDS=0` disables the guards. `VICTORY_OWN_VIEW_HIGH=0` independently disables the 2 TiB floor for Wine views. Both switches still accept false/off and true/on. Tests: `guards_are_on_by_default_on_all_hosts`, `own_views_are_high_by_default_on_all_hosts`.
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

### X4 — `v_div_fixup` resolves zero and infinite operands before a NaN quotient (`victory-shader` `emit/lower.rs`, `emit/live.rs`; Windows feat/live-translator 07f56f5a, from VIctory-NVIDIA feb90ad)

*What.* In the `v_div_fixup_f32` lowering the NaN-quotient case used to override the zero/infinity cases, so `0/x` with a NaN quotient gave Inf (the console gives 0), which bloom/SSR/TAA then spread. Now the quotient-NaN select comes first and `make_zero` / `make_inf` / invalid operand cases override it. `VICTORY_DIV_FIXUP=0` restores the old order (a live switch, `emit/live.rs` whitelist). Hand-merged: only the whitelist line and the switch function of the Windows hunks.
*Tests (mac).* `emit::tests::div_fixup_corner_program_lowers_without_gpu`, `div_fixup_switch_changes_the_translation` (no hardware), `div_fixup_corner_program_executes_on_gpu` (the 12 corner cases on the Mac GPU through the executor of S6; on Windows D3D12), `div_fixup_off_gives_inf_for_zero_over_x_with_a_nan_quotient` (adversarial: `=0` is the old answer). Oracle `oracle_a_valu_f32_misc`: row `v_div_fixup_f32` **1292/1292** with the fix, **45 of 1292** differ with `VICTORY_DIV_FIXUP=0` (the row was a KNOWN bug until this patch; the entry is deleted). Census (`victory-shader` example `translate_count`, 2,631 containers of `research/game-runs/20261128-fidperf/shaders`, census host settings): ok 2,625 / failed 6 before and after, DXIL byte-identical: the title's programs hold no `v_div_fixup` (the opcode census `research/census/opcodes.json` has no `v_div_*`), so the fix is latent for the title.
*Matrix.* `mac.shader.div_fixup_nan_quotient` (proven by the oracle). *Retire when:* mac/livetr merges the Windows line that has it.

### X5 — a one-dword scalar buffer load keeps its bounds check with wide loads (`victory-shader` `emit/lower.rs`, `emit/live.rs`; Windows feat/live-translator d040bcb3, from VIctory-NVIDIA 0745a5d)

*What.* With wide loads on (`VICTORY_SHADER_OPT` WIDE bit, off by default), a one-dword `s_buffer_load_dword` went through the vector-load path and a read past the record returned memory behind it; the console reads 0. `VICTORY_SCALAR_BOUNDS` (default on, `=0` off; live) keeps the bounds check for `count == 1`. Hand-merged: the whitelist line, the switch function and the one condition in `lower.rs`.
*On the Mac chain it changes nothing by itself:* a one-dword load is a single-component `rawBufferLoad` and MSC's `BOUNDS_CHECK` already tests the first dword (the vector case that it does not cover is S13). Evidence: the oracle rows `oracle_mem_smem_one_dword_bounds` (wide on and off: `shader-hw.sh` runs `VICTORY_SHADER_OPT=127` as a second pass) pass; census: default translation of the 300 first corpus programs is byte-identical with the switch on and off, with wide loads on 61 of 300 differ and every program that translates with the switch off translates with it on (299/299).
*Tests (mac).* `s6_corpus_tests::one_dword_scalar_loads_keep_their_bounds` (corpus named by `VICTORY_LIVE_CORPUS`; without it it prints "skipped: corpus not found"; `s6_corpus_tests` is now built on macOS too, its tests skip without the corpus), `div_fixup_keeps_the_translation_pass_count` (the same census check for X4). *Matrix:* `mac.shader.scalar_load_one_dword_bounds`. *Retire when:* mac/livetr merges the Windows line that has it.

### X6 — warm records only pipelines actually made

Windows `feat/live-translator` `ea9f62b6` (`apps/victory/src/warm.rs`). A refused or skipped pipeline remains eligible for a later warm with different settings. `VICTORY_WARM_ONLY_MADE=0` restores unconditional recording. `a_pipeline_left_out_or_refused_is_not_written_down_as_done` counts done entries for 330 successes in 1,000 attempts: 330 rather than 1,000. This is a synthetic fixture, not a frame-time measurement.

### X7 — keep the first parallel scan cut inside the span

Windows `feat/live-translator` `30df9eb4` (`victory-gpu/src/shadow.rs`). Rounding down the start of an unaligned span could wrap past its end; the first cut is now the span start. `VICTORY_SCAN_CUT=0` selects the old arithmetic. Wine test `unaligned_parallel_scan_covers_the_first_piece` checks the first cut, interior alignment and strictly increasing cuts over 20 start/offset pairs.

### X8 — share placed views despite padding; bound ownerless entries

Windows `feat/live-translator` `da60684e` (`victory-gpu/src/descriptor.rs`, `resource.rs`). Fields the title does not initialize must not distinguish otherwise identical views. Bound entries with no resource to release them. Native and Wine tests: `placed_views_ignore_unused_view_words`, `placed_views_share_resources_across_description_padding`, `ownerless_entries_are_capped`, `views_at_new_addresses_add_no_entries`. Controls: `VICTORY_PLACED_PADDING`, `VICTORY_OWNERLESS_VIEWS`.

### X9 — honor the dynamic-slot request on capped heaps

Windows `feat/live-translator` `7d74efb1` (`victory-gpu/src/descriptor/host.rs`). Apply `VICTORY_DYNAMIC_SLOTS` within heap limits and count refused tables in all builds. Wine test `capped_heaps_take_the_asked_dynamic_slots_within_bounds` checks the allocation decision; fewer dropped draws have not been demonstrated.

### X10 — scripted keys also move the first physical pad's sticks

Windows `feat/live-translator` `8e3de411` (`victory-hle/src/modules/gameinput.rs`). Host pad input no longer hides scripted camera/movement keys. Native test `scripted_camera_keys_push_the_right_stick_over_a_real_pad` supplies pad data and checks the scripted right stick. No game or physical input device is required. X6–X10 can be retired when their upstream equivalents are integrated and the offline tests still pass.

### X11 — indirect dispatches up to 2^20 groups in x on the Metal chain (`victory-gpu` `indirect.rs`, `mac/indirect_wide.rs`; our implementation of the upstream pair 8777a39c → f1573eae for NVIDIA/AMD, not cherry-picked)

*What.* `indirect::widest` kept 65,535 groups for a card it does not know, and the Metal chain's adapter (Apple, 0x106B) is one. The title's tile lists are 67,440 and 47,760 groups at 2560x1440, so the first lost its last 1,905 tiles (the lowest band of the picture). On the Metal chain (`VICTORY_METAL_OUT` set; `mac::on_metal_chain`) the bound is 2^20, the clamp's own bound on the total; `VICTORY_INDIRECT_WIDE=0` keeps 65,535. *Why safe.* The back passes the record to `dispatchThreadgroupsWithIndirectBuffer` as it is and Metal takes 32-bit counts: `metal/back/tests/exec_indirect_wide.rs` runs 65,535, 67,440, 2^20 and 3,000,000 groups on an M5 Max and counts every group. Nothing between victory-gpu and the back clamps (front, bridge, back checked, 2026-11-28). *Test.* `mac::indirect_wide::tests`. The D3DMetal path is not touched (no `VICTORY_METAL_OUT`).

### X12 — count DispatchMeshX (slot 98) argument shapes without changing playback

With `VICTORY_MAC_DROP_STATS=1`, record flags, destination presence and payload presence per present. A nonzero destination is still dropped, never read as already prepared dispatch arguments. The slot test checks both destination cases and direct dispatch. Source: `mac/dispatch-mesh-x` at `34691f12`. Retire when the producer and payload have a verified implementation with equivalent telemetry.

### S5 — a mesh program as a compute program and a fetch vertex program: the `VMCS` container part (`victory-shader` `emit/{mesh_cs,lower,shader,mod,host}.rs`, `victory-shader-dylib/src/lib.rs`, `victory-gpu` `host.rs`, `mac/mesh_compute.rs`)

*What.* The translator can make two more DXIL containers of a mesh program, for a host that cannot build the mesh pipeline
(interface v1, `docs/metal/mesh-compute.md` section 8, owned by the Metal back):
* the **compute form**: the same program as `cs_6_6` with the mesh program's `numthreads`; `setMeshOutputCounts`, `emitIndices` and
  the vertex and primitive outputs become `rawBufferStore`s to a scratch buffer (UAV `u0`, space 4000000000); the group's slot in the
  scratch is `g × group_stride` with `g = id.x + dim_x × (id.y + dim_y × id.z)`, the dimensions a root CBV (`b0`, same space, 16 bytes
  `{x, y, z, 0}`, declared in the VS too, unread there); a lane writes only its own vertex and primitive, so the counts are the
  declared maxima cut to the thread count; a culled primitive keeps indices 0 as in the mesh form;
* the **fetch vertex program**: `vs_6_6` with only `SV_VertexID` (`3p + corner`) and `SV_InstanceID` (`g`); it reads the counts and
  the primitive's index word from the scratch (SRV `t0`, same space), and a primitive past the count or with an index past the vertex count
  returns zeros (a triangle without area); every vertex and primitive element is returned (per-primitive elements, S1's added ones too,
  as constant); no cull-primitive element exists in the scratch.
Both go, with the `vmcl 1` layout text, into a `VMCS` part (version 1, flags 0) appended to the mesh shader's own container (hash recomputed);
the other parts, so the mesh pipeline, are byte for byte what they were (test `nothing_is_attached_unless_the_host_asks`). A program that
cannot be lowered (payload or object stage, more than 256 vertices, not triangles, or anything the lowering refuses) gets no part and no error.
Policy (host asks): `emit::set_host_mesh_compute(Off | Problem | All)` (tools: `VICTORY_MESH_COMPUTE=0|1|2`), default **Off**; Problem = only
programs whose Metal mesh struct is over 32 KiB. It travels in `host_settings()` bits 29-30, so the shader cache and `victory-shader-dylib` key
on it. `emit::translate_mesh_compute` returns the products without a mode (census, tests).
*Host.* victory-gpu tells the translator at host creation (`mac/mesh_compute.rs`, one hook line in `host.rs` beside S1's): **Problem on the Metal chain when the back says it runs the part
(`VICTORY_METAL_MESH_COMPUTE_SUPPORTED=1`, exported by `tools/mac/run.sh` when the stage's `CAPS` file has a line `mesh_compute`), otherwise Off, and Off on
D3DMetal**; `VICTORY_MESH_COMPUTE=0|1|2` overrides. Why the gate: the back's denylists (`msc-denylist`, `mesh-pair-denylist`) name a stage by the FNV hash of its whole
container, which the `VMCS` part changes: 966 of the 1273 pairs of the pair denylist have a mesh that gets the part in Problem mode, so a back that does not
hash without the part must not be given it (the converter itself ignores the part: 442 mesh containers built identically with and without it on the back before mc-back). The chain is known by `VICTORY_METAL_OUT`, which `tools/mac/run.sh --backend metal` exports to the title (nothing
else the exe sees tells the two apart: S1's `host_attribute_at_vertex` is false on both). Tests: `mac::mesh_compute::tests`.
*Not done (decided to wait for the performance phase).* Pruning the outputs the pixel program does not read: the translation of a mesh shader does not see its
pixel shader, so it needs the host to pass the pixel program's (name, index) reads next to S1's `vertex_reads`, and the layout key `reads MASK` (not written now, meaning all).
Hook lines in upstream files are marked `mac S5`. Tests: `emit/mesh_cs_tests.rs` (13 unit tests; hardware tests under Wine
`the_compute_and_fetch_draw_is_the_mesh_pipelines_draw`, `mesh_compute_form_draws_flagged_triangles_and_drops_culled_ones`: the compute
pass and the fetch draw give the same texels as the native mesh path, dispatch sizes 1x1x1, 2x1x1, 1x3x1, 2x2x2).

*Why not needed on Windows.* D3D12 mesh pipelines have no output-size limit that matters; Metal's mesh stage has 32 KiB of output per group
(1261 of the title's real mesh pipelines are over it; Apple's GPU compiler crashes on 1400 more, see S1), so the Metal back runs such a mesh shader
as a compute pass and a vertex draw (docs/metal/mesh-compute.md).

*Remove when.* Metal's mesh stage takes the title's meshes (size and compiler), or the back no longer emulates mesh pipelines: drop `mesh_cs.rs`, the
`cs` form of `Parts` and the hooks (`grep 'mac S5'`).

*Acceptance (2026-11-28).* Census (`victory-census mscs`, branch `mc/census-s5` of the Metal back at 242f1925 + this patch; data under `victory-data/census-s5/`):
**A, every mesh program of the package (8765 containers, each translated alone, mode All)**: 8765 get a `VMCS` part; the fetch vertex
program builds a Metal render pipeline (through MSC, with the default pixel shader) in 8765; the compute form builds a compute pipeline in
7536 with the root signature the back makes today and in **8765 with the static samplers visible to all stages**: the 1229 others are all
one error, `ResourceNotReferencedByRootSignature (SMP space 0 register 0)`, a fault of the back (`mesh_emu::extend_root` makes the
parameters visible to the compute stage but leaves the static samplers at the mesh stage's visibility), not of the translator.
Of the 520 mesh programs whose native mesh pipeline Metal refuses for the 32 KiB, all 520 have both forms and build (367 with today's root, the other 153 after the sampler fix).
**B, the 9050 (mesh, pixel) pairs of the pipeline book** (mesh translated with the pixel program's S1 reads, the fetch vertex program paired with
the real pixel program): compute 9050, fetch + pixel pipeline 9047; the 3 others (`06980160f6c5516f:2f3b5ef239a76315`, `52b873d99483b41b:81b24241b09514d6`,
`ab008fdce9f216c6:81b24241b09514d6`) are pairs whose pixel program already crashes Apple's compiler on the native mesh path without S1 (the "3 that crash without S1"
above), they crash in the vertex form too (XPC_ERROR_CONNECTION_INTERRUPTED, reproduced). The prototype sample was 200/200 and 199/200.
Tests: `emit/mesh_cs_tests.rs` 13 unit tests; Wine/D3DMetal: the compute pass plus the fetch draw gives the texels of the native mesh pipeline
(dispatch 1x1x1, 2x1x1, 1x3x1, 2x2x2) and the flagged-triangle / cull / out-of-range-index test passes.

### S6 — victory-shader's hardware proofs run on the Mac GPU (`victory-shader/src/emit/{host_exec,validator_none}.rs`, hooks in `emit/{mod,host,valu_tests,permlane_tests,waterfall_tests,cube_lod_tests}.rs`, `tools/mac/shader-hw*.{sh,py,tsv}`)

*What.* The tests of `valu`, `wave64`, `permlane`, `waterfall`, `intersect`, `size`, `cube_lod` were written for the D3D12 host (`emit/host.rs`, Windows only), so on a Mac they did not exist. On macOS (test builds only) `emit::host` is now `host_exec.rs`: the same `Host` API (`new`, `compute_pipeline`, `compute_pipeline_with`, `dispatch`, `Dispatch/View/Root/Stride`, the `api` names of D3D12/DXGI) that sends the DXIL and the bindings over a pipe to the executor `victory-shader-exec` (`metal/shader-exec` of the Metal back, branch `harness-back`; DXIL -> Metal Shader Converter -> Metal 4 compute -> read back) named by `VICTORY_SHADER_EXEC`. No executor: `Host::new` fails and every test skips, so `tools/mac/check.sh` is unchanged. This crate names no Metal symbol and depends on no `victory-metal-*` crate (layering); the pipe protocol (`wire`, version 1) is duplicated on both sides and pinned by one golden vector each. Hooks in shared code: the `#[cfg(windows)]` gates of the compute test modules become `any(windows, macos)`; the tests ask the device through `Host::caps()` (wave sizes, AMD, mesh) instead of `host.device.CheckFeatureSupport` (the Windows host got the same `caps()` and `api` module); a hardware `compute_pipeline` error that starts with "unsupported" skips (f64). `tools/mac/shader-hw.sh --back <metal/ dir>` builds the executor, runs the lib tests and tallies passed / failed / skipped; `shader-hw-known.txt` classes the known failures (base / semantic / bug). The per-mnemonic opcode oracle (`emit/oracle_tests.rs`, `gfx10::opcode_of`, helper visibility in `valu_tests.rs`; `docs/mac/opcode-oracle.md`) is on the same host: 140 table rows in ten families against a CPU reference, run on whatever device the tests have. The executor also draws (vertex/geometry/mesh + pixel pipelines through the back's `graphics_pipeline_for_host`; `Host::{pipeline,pipeline_with_gs,mesh_pipeline_with,draw}`), so `rectangles_are_drawn_whole`, the mesh edge-flag tests and `quad_neighbours_…` run on the Mac GPU. `View::Depth` (a D32_FLOAT texture read as R32_FLOAT) is the binding for a comparison sampler (`image_sample_c*`); the test `comparison_sampler_reads_a_depth_texture`.
*C-oracle follow-up (shared translator).* The C rows exposed a silently skipped local call and three rejected FP16 colour exports. `gfx10::local_calls` preserves the two external vertex-fetch ABI forms, resolves a constant PC-relative target with a straight-line VALU body and a matching return, and refuses unknown calls. `emit/lower.rs` unpacks proven FP16 pairs, not arbitrary 16-bit render-target formats. Both corrections are host-independent and removable when upstream provides equivalent lowering. On the title corpus no call-containing container's translated DXIL changed and no compressed export survives the existing PS patch pass. See `docs/mac/opcode-oracle.md`.

*Why not needed on Windows.* Windows has D3D12 and the DXIL validator; the tests run there as they were.
*Remove when.* A Windows-independent device for the proofs exists upstream (or the Windows host is rebuilt on the same pipe).

### S7 — 16-bit float results rounded to nearest by default (`victory-shader` `emit/{lower,bitcode,shader,live,mod}.rs`, `victory-shader-dylib/src/lib.rs`, `victory-gpu` `host.rs`, `shader_cache.rs`, `mac/metal_chain.rs`)

*Formerly called F1.* The environment variable names are unchanged (`VICTORY_F16_NEAREST`, `VICTORY_F16_NATIVE`); the hook lines in the source are marked `mac S7` (they were `mac F1`).

*What.* The translator's `VICTORY_F16_NEAREST` (round the result of a 16-bit float operation to nearest even, `Lower::f16_nearest`) is **on by
default**; `VICTORY_F16_NEAREST=0` turns it off. It now covers every result rounding of the title's code: `v_cvt_f16_f32`, the VOP f16 arithmetic
(`v_add/sub/mul/fma/fmac/mad/min/max/med3/rcp... _f16`, `v_pk_*_f16`), `v_fma_mixlo/mixhi_f16` and `v_interp_p2_f16` (these last three used the host's
plain `legacyF32ToF16`; one helper, `Lower::f16_result`, is now the only place that picks). Not touched: `v_cvt_pkrtz_f16_f32` (round toward zero on
the console too, `f16_toward_zero`), the format conversions of d16 stores, and the wave sum of halves (already exact). The shader cache key holds the
effective state (`emit::f16_nearest()`), as for `VICTORY_EXPORT_F16_SAT`, so a default flip or a `=0` run never reuses the other's translations.

*One `fptrunc` on our own Metal chain (`VICTORY_F16_NATIVE`, default on only there).* "Our own Metal chain" is decided as S5 decides it: `VICTORY_METAL_OUT` set and not empty (`run.sh` exports it on `--backend metal` only; `victory-gpu` `mac/metal_chain.rs`, told to the translator at host creation by `set_host_metal_chain`, and to the live translator library in bit 28 of the last `host_settings()` word, so the shader cache keys on it; a tool that translates offline without a host falls back to reading `VICTORY_METAL_OUT` itself). It is not the test "the host has no `attributeAtVertex`" (the first form of this patch): D3DMetal has no `attributeAtVertex` either, and D3DMetal is the reference path, so its translation stays as it was (the sequence); `VICTORY_F16_NATIVE=1` forces the native form there by hand. Unit tests `lower::tests::f16_native_is_on_by_default_on_the_metal_chain_only`, `f16_native_follows_the_chain_the_host_says`, `mac::metal_chain::tests`. The
nearest rounding is not the dozen-operation sequence there but one `fptrunc float to half` of a native 16-bit module: LLVM's `fptrunc` rounds to nearest
even, and MSC compiles it to Metal's `half(x)`. The writer got a `half` type, the `fptrunc` cast (opcode 7), the data layout DXC writes for native
16-bit modules (`f16:16`, `i16:16`, `i8:8`), and the Native16 feature (shader flag 1<<23, `SFI0` bit 42) which a module with a `half` sets. The
sequence stays for the other hosts (a D3D12 host without native 16-bit shader ops would refuse the module) and for `VICTORY_F16_NATIVE=0`. Why it was
worth it: the sequence costs +4.4 % DXIL and +17..59 % pipeline build time on the 11,621 changed programs; the `fptrunc` costs **-0.04 %** DXIL (median
-0.05 %, p90 +0.1 %, max +2.4 %) and the pipeline build time is the same as with the switch off (sample of 2,325: cs 166 s vs 174 s off, ps 33 s vs 27 s, ms 56 s vs 62 s,
vs 5.8 s vs 4.9 s; noisy machine). Pass counts unchanged (25,760 translated; sample cs 198/198, vs 165/165, ps 104/1,028, ms 917/934). Hardware: the same tests as
below, with the `fptrunc` path: 0 differences in 134 M-input samples and in 755 M inputs of each of the three instructions (`fptrunc` of f32 to half, 1.24 M inputs
around every rounding tie of all halves also exact in `metal/back/tests/dxop_fptrunc.rs` on the metal line).

*Why not needed on Windows.* The Windows line keeps the switch off (exact but untried in a game run; the upstream comment: the host's `f32tof16` is also
toward zero on NVIDIA); there it is a choice, not a fix. On the Mac, Metal Shader Converter's `f32tof16` is round toward zero (D3D11.3 Functional Spec 3.2.2
allows it) with a double rounding for half denormals: a result that is not exact is 1 ULP (2^-10 relative) lower than the console's, in
about 37 % of f32 inputs (measured: 50,329,600 of 134,217,728 sampled inputs differ from nearest). 11,642 of 26,177 translated programs (44 %)
have such a rounding (11,621 of the 25,756 translated census programs change by it).

*Remove when.* The Windows line turns the same switch on (merge conflict at the `mac S7` lines) or MSC rounds to nearest.

*Evidence (2026-11-28).* Hardware proof on the Mac GPU (Apple M5 Max, macOS 27.2.1, MSC 4.0.3, through the S6 executor; test
`emit::f16_hw_tests`, branch `mac/f16-hw-proof` over `mac/shader-harness`): the real translator output of `v_cvt_f16_f32`, `v_fma_mixlo_f16`
(f32 operands) and `v_add_f16` against the exact CPU nearest-even rounding, 1.87 billion inputs of each (111 of the 256 chunks of 2^24: every
exponent range of both signs, denormals, 65520, Inf, NaN, -0): **0 differences**, NaN gives NaN, an f32 denormal input may give either zero; the same programs with the switch off differ in 37 % of inputs, so the test can fail. Census (`victory-census translate`, 25,985
containers): translated 25,760 on and off, failed 225 on and off, the same programs; 11,621 translations change; DXIL bytes of those +4.4 % in
total (median +6 %, 90th percentile +14 %, largest 2.9x). MSC/Metal on a 20 % sample of the changed programs (2,325): cs 198/198, vs 165/165,
ps 104/1,028, ms 917/934 on **and** off, the same programs; with the 12-operation sequence the pipeline build time of the sample (a loaded machine, noisy) is cs +57 %, vs +17 %,
ps +59 %, ms +43 % (the `fptrunc` form: none, above). GPU run time of the added ~12 operations per result is not measured. Unit test `lower::tests::f16_nearest_is_on_unless_switched_off`.

### S9 — a `v_cndmask_b32` whose mask is a plain number counts for the console-lane analysis (`victory-shader` `emit/lower.rs`, `selects_by_number_mask`)

*What.* A `v_cndmask_b32` whose scalar mask is a plain number (`s_mov_b32 vcc_lo, sN`, a root constant, a loaded dword; not a lane mask the structuring tracks) now counts for the `console_lanes` pre-analysis. Before, a compute program with a thread count tripped `debug_assert!(threads.is_none() || console_lanes)` in `cross_lane` (in a release build it used the host lane number unrelabelled: a silently wrong select). Test `a_scalar_move_into_vcc_translates_in_a_thread_limited_program`; oracle rows in `emit/oracle_a_tests.rs` (`docs/mac/opcode-oracle.md`). Found by the opcode oracle's A batch.
*Why not needed on Windows.* As wrong there as here, but no Windows run has met it (the debug assertion is a test build's); the same code serves both, so the fix is harmless there.
*Remove when.* Never needed: upstream may take it as it is.

### S10 — `v_writelane_b32` masks its lane operand (`victory-shader` `emit/lower.rs`)

*What.* The lane is S1[4:0] (wave32) or S1[5:0] (wave64), as the ISA reads it (the operand is masked with 31 or 63); a number past the wave named no lane and wrote none. Oracle rows in `emit/oracle_a_tests.rs`.
*Why not needed on Windows.* As wrong there; no Windows run has met it. *Remove when.* Never needed: upstream may take it as it is.

### S11 — `v_sin_f32`/`v_cos_f32` (and the f16 forms) of NaN or infinity give NaN (`victory-shader` `emit/lower.rs`, `trig_turns`)

*What.* An infinity or NaN argument gives NaN (`x - floor(x)` is NaN there; the host's sin(NaN) is 0 through MSC/Metal). The two `semantic` rows of `tools/mac/shader-hw-known.txt` for `v_sin_and_v_cos_*_reduce_large_turns` are gone. Oracle rows in `emit/oracle_a_tests.rs`.
*Why not needed on Windows.* D3D12's sin(NaN) is NaN already; the same code serves both. *Remove when.* Never needed.

### S12 — `s_wqm_b32`, `s_wqm_b64`, `s_bfm_b64` of a plain number translate (`victory-shader` `emit/lower.rs`)

*What.* They were "Unsupported" when the operand is a number rather than a lane mask (4,088 + 19,646 + 390 instructions in the title; the wqm ones are mostly lane masks, which keep their own path in `structure.rs`). `s_wqm` is the quad-wise OR of a number; `s_bfm_b64` is `((1 << S0[5:0]) - 1) << S1[5:0]`. Oracle rows in `emit/oracle_a_tests.rs`.
*Why not needed on Windows.* Not known to be met by a Windows run (not measured); the same code serves both. *Remove when.* Never needed.

### S13 — vector buffer loads past a view's end read 0 per dword; `global_load_ubyte/sbyte/ushort/sshort` of title memory (`victory-shader/src/emit/lower.rs`: `zero_past_end`, `raw_dwords`, `format_load`, `buffer()`, `memory_flat`)

*What.* A vector `rawBufferLoad` (`buffer_load_dwordx2/x3/x4`, `tbuffer_load_format_*` of 64-bit and wider formats, `s_buffer_load_dwordx*` through `raw_dwords`) that starts inside a view and ends past it returned the memory behind the view: MSC's `BOUNDS_CHECK` tests only the first dword. The size is now read (`getDimensions`) and component k >= 1 is selected to 0 unless `byte + 4k + 4 <= size` (and the address does not wrap). This also fixes `scalar_buffer_loads_read_the_same_dwords_one_call_or_four` at `VICTORY_SHADER_OPT=127` (removed from `shader-hw-known.txt`). Not covered: structured-stride loads, title memory by address (`global_load_dwordx*`, `s_load_*` of memory read through pages, not views). Also (part 2 of the same entry) the four sub-dword `global_load_*` of title memory translate (the dword read, shifted out as the root-view path already did).
*Why not needed on Windows.* D3D12 zeroes out-of-range per component.
*Remove when.* MSC bounds-checks each component of a vector raw load. Proof: `oracle_mem_*`, `probe_straddle_past_the_view_end`, `oracle_flat_loads`.

### S16 — bounded local shader calls and compressed FP16 colour exports

Local PC-relative calls execute the callee and return; unresolved indirect forms are rejected, not silently skipped. Title vertex-fetch handovers remain inputs. Decode packed half pairs for compressed colour export. Source: `mac/shader-pc-oracle` at `c722f6f8`. On the 2,631-container fidperf corpus, before and after: 2,625 translated, the same 6 failed, aggregate DXIL digest `73cba1c214599504`. Retire when the shared translator passes the call/return and export oracle rows.

### S15 — expand live EXEC lanes per quad for scalar `s_wqm` (Windows ledger S21)

A quad is active only if one of its four source lanes is active, not unconditionally. The EXEC oracle compares wave32 and wave64 with the ISA reference (300/300 each). Distinct from S12's scalar-number operand. Retire when the common lowering has the same semantics and passes both reference comparisons.

### S14 — a fence after a counter-texture `image_store` (`victory-shader/src/emit/lower.rs`, `image()`)

*What.* The upscaler's counter texture (`image_store` of 9, then `image_atomic_add 1` on the same texel by the same thread) gave 9, not 10: Metal does not order a thread's texture write against its later texture atomic. A mode-2 `barrier` (the existing `memory_fence`) after the `textureStore` of such a counter store makes the store land first; 6/6 runs give 10 (`size_tests::a_counter_texture_is_set_by_a_store_of_one_channel`, removed from `shader-hw-known.txt`).
*Why not needed on Windows.* D3D12 orders a thread's UAV accesses to one texel.
*Scope.* Only the one-channel store into a view the same program also counts in with an integer atomic (`is_counter`); ordinary `image_store` gets no barrier. Census of the game package (25,985 containers, 2,447 with `image_store`, 7,635 `image_store` instructions): 1 container is affected, 3 fences emitted (2 store instructions; the lowering runs the store path more than once); 20 containers hold both a store and an atomic, 19 of them do not take the counter path.
*Remove when.* MSC/Metal order same-thread read_write texture accesses.

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

### S8 — a 64-bit image atomic max that returns the original: read first, atomic without its result (`victory-shader` `emit/lower.rs`, `emit/live.rs`, `victory-gpu` `shader_cache.rs`)

*Formerly called F2.* The environment variable name is unchanged (`VICTORY_F2_READ_FIRST`); the hook lines in the source are marked `mac S8` (they were `mac F2`).

*What.* `image_atomic_{s,u}{min,max}` of two channels (an R32G32 view as one 64-bit texel) with GLC lowers as a `textureLoad.i32` of the texel, joined
to 64 bits, then the `atomicBinOp.i64` whose result is not used; the load's value is the register result. Other 64-bit atomics are as before.
`VICTORY_F2_READ_FIRST=0` turns it off (a key of the shader cache).

*Why not needed on Windows.* D3D12 drivers return the original of a 64-bit atomic. MSC refuses the instruction with its result used
(`UnsupportedInstruction: dx.op.atomicBinOp.i64`) and takes it without; Apple GPUs do 64-bit min/max natively (measured on the M5 Max, `docs/metal/dxop.md`
on the metal line). Between the load and the atomic another lane can raise the texel: the load is then older than the atomic's own original. The title's only
program that uses the return (`PS_Weather_Particle_Prepass_74a7a22c`) takes the high half as a depth for a `discard` against its own depth (a max only goes up):
a stale load can only miss a discard that a later pixel would make anyway. That is a reading of the program, not a measurement.

*Remove when.* MSC returns the original of a 64-bit min/max, or Metal does.

*Evidence (2026-11-28).* Unit test `emit::f2_tests` (the load is there with GLC, not without or with the switch off). Census of the 679 programs that declare a
64-bit atomic (677 ps, 2 cs), translated and converted with MSC 4.0.3 with the switch on and off: ps 61 ok / 616 refused on, 60 / 617 off (the 616 are the
`attributeAtVertex` refusals, which S1 handles on the host; the census converts without it), cs 2/2 both; the program that flips is 64974027a11daa59, the weather prepass. The read-then-max form
itself ran on Metal in `metal/back/tests/dxop_atomic64.rs` (the metal line). The translated program's run (62 convert) is the mac-host-harness line's.

### M7 — `tools/mac/play.sh`: hand play wrapper

*What.* A 20-line wrapper over `run.sh`: perf (since 2026-11-28; `--fidelity` is an option), no `VICTORY_KEYS`, no input replay, watchdog off (L5), 4 h limit; defaults `--exe` and
`--prefix` to the main checkout's when this worktree has no build. Docs: RUN.md "Playing by hand". `run.sh --worldsave` / `--new-game` and the refusal of a seed with a world save (2026-11-28): new game is the default, the world save is only on request. *Remove when.* run.sh grows a play preset.

### M13 — `tools/mac/accept-batch.sh`: one-command batch acceptance (data only)

*What.* After each merged block, one unattended game round through `run.sh` (`--backend metal --perf --new-game --no-watchdog --frametime-log --until-world 300 --timeout 720`, default keys and input, `VICTORY_MAC_DROP_STATS=2`, `VICTORY_METAL_DROPSTATS=<run>/dropstats`) and a `summary.md` / `summary.json` under `research/accept/<time>-<short mac/livetr commit>/`: version (commit, stage, exe sha, `VICTORY_WAVE_WIDE`, the MSC denylist), alive or not, highest world frame, time to world frame 1, frame-time median/p90 and frames over 5 s over world frames 1..300, load, drops of the shared layer (G8, classes of `drop-stats-report.sh`) and of the Metal back, top 10 reasons of each, and the difference to the last acceptance. No screenshots, no capture runs. `tools/mac/test-accept-batch.sh` tests it with a fake Wine. `run.sh` is used as it is (no option added). Docs: RUN.md "Batch acceptance". *Remove when.* the Metal side's own acceptance tool replaces it.

### M14 — `tools/mac/census-title-mnemonics.sh`, `tools/mac/census-isa-coverage.sh`: instruction forms and how much of them the Rosetta probes prove

*What.* `census-title-mnemonics.sh` counts every instruction form (mnemonic plus operand shape: `vaddps y,y,m256`) of the title's `.text` (about a minute; 26.7 M instructions, 1,482 forms, 510 mnemonics on build 1.0.2.1), the complement of `census-title-isa.sh` which counts only the Rosetta-relevant classes. `census-isa-coverage.sh` sorts the forms into categories and into three tiers (proven by a probe case, partial, unproven; static presence only, execution is unknown for all, so used=unknown) and writes `coverage.tsv` / `coverage.txt`. Output in `research/census/title-mnemonics-<date>/` (game-derived, not in the repo). CPU only, no game. *Remove when.* the matrix carries one row per category with the numbers (then these are the regression for a new title build).

### M16 — GPU error count in the run summary

Count case-sensitive error tokens once per line across both logs. Keep complete kIOGPU and DXGI device-error identifiers; ignore the watchdog-disabled startup message. Retire when the summary reads structured GPU error events instead of log text.

### M15 — `tools/mac/hle-functions.py` + `docs/mac/hle-functions.csv.txt`: per-function HLE state of the title's import table

*What.* Static (no game, no Windows): reads `research/census/title-isa-<date>/import-callsites.tsv` (588 rows: 587 unique dll+function, 61 of them also address-taken), the HLE registry as written in `crates/victory-hle/src/modules/*.rs`, `HostPolicy::default()` and the export tables of Wine's builtin DLLs (`target/wine-cx`, API sets resolved to kernelbase/kernel32/ntdll/ucrtbase/user32), and writes one row per function: implemented / stub / passthrough-to-host / missing / title-dll (shipped by the title: Bink, libHttpClient, XboxBvhBuilder, the FidelityFX loader). Stub is a heuristic on the source (doc comment says stub/shortcut/not implemented, or a body that calls nothing); the `basis` column says why. `runtime_calls` stays empty until `mac.runtime.hle_counters` exists and one run has been made. Result 2026-11-28: 85 implemented, 8 stub, 422 passthrough (all exported by Wine), 72 title-dll, 0 missing. *Remove when.* the matrix itself carries per-function rows.
### X13 — the Windows line's 1440p chain: native 1440p output, scene scale, scale watch, frame rate; the perf profile takes it by default (`victory-loader` `mac/output_chain.rs`, `render_scale.rs`, `frame_rate.rs`, `apps/victory/src/native_1440p.rs`, `run.rs`, `victory-gpu` `draw.rs`, `frame.rs`, `tools/mac/run.sh`)

*Source.* `feat/live-translator` of the Windows line, six commits in this order: `30e991fc` (`VICTORY_FRAME_INTERVAL_MS`, `VICTORY_TITLE_FPS`),
`0c1905bf` (`VICTORY_NATIVE_1440P`, `VICTORY_RENDER_1080P`; from DexNom 9a79132), `72eb7c6c` (`VICTORY_RENDER_SCALE` 0.25..1, pinned),
`063cdc0a` (the scale is fixed for a run), `72f115b5` (`VICTORY_SCALE_WATCH`), `0a4e75db` (its result, no code change); plus a test-only commit that
makes the watch test run under Wine (wrapping address arithmetic, a high test page). Applied with `git am` from the trial export (conflicts
resolved by hand in `lib.rs`, `run.rs`, `draw.rs`: see the README of the trial); one `mac:` commit each, source SHA in the message. Author set to the placeholder identity.
Three doc comments in the trial patches had their `→` turned into `??`; put back.

*What the chain does to the title* (image offsets, build 1.0.2.1 only; every field compared against the stock bytes, another build is refused).
`native_1440p::prepare` writes the **prepared image** before it is mapped (a folder of its own under `images/`, one per scale and pin):

| field | at | length | stock | written |
|---|---|---|---|---|
| SDR display mode record (5th of the mode table at +0x0615c0c0) | +0x0615c100 | 16 | 3840, 2160, 60, 1 | 2560, 1440 (the one place the title's 3840x2160 comes from; checked on the title image by the byte test) |
| scanout reservation (`mov ecx, imm32`) | +0x0247ad4a | 5 | `b9 00 00 00 04` (64 MB) | the size of two 64 KiB-aligned 1440p RGBA8 scanout buffers, rounded to 2 MiB (what the back buffer pool's left-over check at +0x247af25 wants used up) |
| temporal scene scale minimum (imm32 of `mov dword [rcx+90h], 2/3`) | +0x028f1e7e | 4 | 2/3 | the scale (1.0 default; 0.75 for `VICTORY_RENDER_1080P`) |
| scale controller `vmaxss xmm0, [rsi+0x90]` | +0x028f1e87 | 8 | `c5 fa 5f 86 90 00 00 00` | `vmovss` (pinned) only for `VICTORY_RENDER_SCALE` (`VICTORY_RENDER_SCALE_PIN=0` leaves it) |
| Settings preview viewport load | +0x058048d4 | 8 | 3840x2160 constant | the SDR record |

Runtime: `VICTORY_SCALE_WATCH` (default on with the 1440p image, `=0` off) puts a detour on the `vmaxss` and logs min/median/max of the scale the title computes every 10 s;
`VICTORY_DISPLAY_MODE=2560x1440` is set for the HLE; `victory-gpu` `State::native_frontend_viewport` draws the frontend's last triangle with a 3840x2160 viewport when it still uses the legacy 4K
projection (two shaders), scissor and target stay 1440p. `VICTORY_FRAME_INTERVAL_MS` (off) ticks our frame clock and rewrites the two loads of the title's 30 fps setting
(+0x1e0fc62, +0x224359a; the commit message says 3, the code has 2).

*The decision: perf takes native 1440p, fidelity does not; the switch follows the profile* (`victory-loader/src/mac/output_chain.rs`, decided once in `run.rs`, printed as `output chain: ...`):

| profile (from `VICTORY_MAC_PROFILE`, exported by `run.sh`; else `VICTORY_OUTPUT` 0/none = fidelity, a size = perf; else a Mac is perf, a PC has none) | native 1440p |
|---|---|
| fidelity | **always off**, nothing about the output patched. `VICTORY_NATIVE_1440P=1` or `VICTORY_RENDER_1080P=1` are ignored with the line `output chain: native 1440p stays off: the fidelity profile keeps the title's own 4K output (... ignored)`. The victory-gpu variable is set to 0 so its frontend viewport cannot turn on by itself. |
| perf, nothing set | **on** (`the perf profile's default`), scale 1.0 unpinned, L3's output not asked |
| perf, `VICTORY_NATIVE_1440P=0` | off; L3's 1280x720 default (a Mac) applies |
| perf, `VICTORY_OUTPUT=720` (also `720p`, `1280x720`, any of the title's modes, short forms by height) | off; the L3 chain with that size |
| perf, `VICTORY_NATIVE_1440P=1` and `VICTORY_OUTPUT=<size>` | on; the L3 output is dropped, the line says so |
| perf, `VICTORY_RENDER_1080P=1` or a readable `VICTORY_RENDER_SCALE` (0.25..1) and no output size | on, at that scale (pinned for `RENDER_SCALE`, as on the Windows line) |
| perf, not a Series X (`--console series-s`) | off with a line (native 1440p is for the Series X image); asked outright (`=1`) `run.rs` refuses as before |
| no profile, not a Mac (a PC) | off unless asked (`=1`, `RENDER_1080P`, `RENDER_SCALE`), as on the Windows line |

Precedence for native 1440p: fidelity > explicit `VICTORY_NATIVE_1440P` > explicit `VICTORY_OUTPUT` size > a scale asked > the profile's default. Wrong values are said and ignored.
`run.sh` exports `VICTORY_MAC_PROFILE`, no longer forces `VICTORY_OUTPUT=1280x720` in perf, and says `output chain ...` in `start.txt` (it only predicts; the title's line is the record).

*Relation to L3 (720p): alternatives, not layers.* Reasons: (1) L3's output patch writes 1280x720 into the settings that the title hands to the screen-size setter, native 1440p
makes the display mode record and the scanout buffers 1440p: both on would present a 720p screen size over a 1440p mode and scanout (never run; the order in which the title reads the two is not known).
(2) L3's pool check jump exists because a smaller output leaves the 4K-sized pool unused; native 1440p resizes the pool itself (scanout reservation), so the jump is no longer
needed. (3) **L3's scale site and the native minimum overlap**: L3's `OWN_SCALE` is `mov dword [rcx+90h], imm32` at +0x28f1e78 (10 bytes), its immediate (+0x28f1e7e, 4 bytes) is exactly the native `TEMPORAL_SCALE`.
Written after the native patch, L3's byte comparison sees the native value and refuses the site (`Differs`), written before, it would be overwritten. (The trial's byte check
called the L3 and native groups disjoint; it compared the output and pool check sites, not the scale site.) So with native 1440p on, L3 asks for **no output, no pool check with it and no scale**;
what stays available is L3's upscaler picker (`VICTORY_UPSCALER`), an explicit `VICTORY_POOL_CHECK=skip` and the divide clamp, all disjoint from the native fields. L3's 720p stays as the
optional chain (`VICTORY_OUTPUT=720`, or `VICTORY_NATIVE_1440P=0` on a Mac). Not changed: the fidelity profile (no site patched).

*Cost of the default.* Native 1440p at scale 1.0 gives an internal size of **2560x1440**, the same as fidelity (4K output x 2/3); the old perf profile was 853x480 (720p x 2/3), 9x fewer pixels:
GPU time of a world frame was 8.4 s at the title's own 4K output and 0.86 s at 720p (macos.md). Perf is therefore expected to be much slower than before unless a scale is given:
`VICTORY_RENDER_SCALE=0.333` pins 853x480 (not run; the Windows runs were at 1.0, 0.667 and 0.5), `0.5` gives 1280x720, `0.667` gives 1707x960.

*Tests.* Native (`cargo test`): `mac::output_chain` 11 tests (perf default, a Mac without `run.sh`, a PC, profile from `VICTORY_OUTPUT`, fidelity forced off with the log line, `=0`, output sizes and
short forms, native over an output size, scales, Series S, switch words); `victory native_1440p`: `native_and_l3_sites_never_overlap_in_any_switch_combination` (3 profiles x 3 native x 2 x 3 scales x 6 outputs x
pool check x 3 upscalers: the sites L3 writes, the native ones and the frame rate loads are pairwise disjoint, and native 1440p never comes with L3's output or scale site),
`the_only_overlap_of_the_full_site_lists_is_the_scale_immediate` (every site L3 can write against every native range: exactly one pair, 4 bytes, the one above),
`the_native_and_l3_bytes_on_the_title_image` (reads the title image, read only: path `VICTORY_DISPLAY_PROFILE_TEST_IMAGE`, else `$VICTORY_GAME_DIR/game_scarlett_master.exe`, else skipped with a line;
stock bytes of every L3 site, native field and frame rate load; the five-entry mode table; four both-on combinations: every L3 group is written over the native image and leaves the native fields as they are; the
scanout reservation holds two 1440p buffers and is under the 4K pool; fidelity plans no site; L3's scale site over the native image is refused), and the Windows line's `patches_the_retail_image_for_1440p`, which now
runs for real with the same image. Wine (`tools/mac/wine-tests.sh -p victory-loader --lib`): the chain's `render_scale` and `frame_rate` tests (6, with `the_scale_watch_notes_what_the_instruction_made` that runs the detour on the CPU) and `output_chain`, `resolution`: 32 passed.
Not run: the title (not here), so no node below is proven.

*The UI, offline.* The title lays its UI out in fixed 4K pixel units (matrix `mac.res.ui_layout`): the hint bar is 604x54 px with 81 px / 60 px margins at the output's right and bottom edge in
every measured output (3840x2160: 15.7% of the width, centre 0.900, 0.960; 1280x720: 47.2%, centre 0.701, 0.880 = 3840/1280 = 3.0x). Both centres are reproduced exactly by "constant pixel size,
anchored to the output edges" (`(W - 81 - 302) / W`, `(H - 60 - 27) / H`), so the layout reads the output size for the anchors and not for the size. In those runs the SDR mode record was always 3840x2160 and only the output changed
(L3), so they cannot say whether the size follows the record. With the record at 1440p the two readings give: **(U1) UI size constant in pixels, 1.5x** (3840/2560): 604x54 px = 23.6% of the width, centre (0.850, 0.940);
**(U2) UI size follows the record's height (x 1440/2160)**: 403x36 px = 15.7%, centre (0.900, 0.960), 1.0x, the same picture as the fidelity run shown at 1440p (P1: 402x36 px). Nothing in the repository separates them; the
Windows line's remark "no 4K pass and no 4K UI" is about the cost of the 4K surfaces, not about the layout. A third reading comes from the frontend's last triangle: where it still projects with the legacy 4K matrix,
the viewport patch makes that draw 1.0x. **Expected: 1.5x for the hint bar** (U1, the only reading the measured law supports without an extra assumption); the chain then halves the problem (3.0x to 1.5x) but does not fix it, and a UI scale
site would still be needed (not found: not settings `uiResolution`, not the HLE display mode). The run decides; the numbers to compare are in the matrix row `mac.res.ui_scale`.

*Does the display mode record decide the output? (the matrix's "H1 disproved" against the Windows ledger's P2).* Two different things are called "display mode".
(a) The mode the **HLE reports** to the title (`victory-gpu` `dxgi.rs`, `VICTORY_DISPLAY_MODE`; matrix `mac.res.display_mode`): our run series 2026-11-28 (A,B,C,D1,D2,E, with 2560x1440, 1920x1080, 1280x720 and unset) showed that it does **not** decide the output
(the matrix's "H1 (display mode decides) disproved"; not this file's patch H1). (b) The title's **own SDR display mode record** in its image (+0x0615c100, 5th entry of its table +0x0615c0c0, 3840x2160 60 Hz), which no run of ours ever changed.
`0c1905bf` does not report a 1440p mode to get 1440p: it writes (b) (with the scanout reservation, the scene scale and the Settings viewport) into the image, and `run.rs` additionally sets (a) `VICTORY_DISPLAY_MODE=2560x1440`.
The Windows ledger's wording ("a switch that reports a 1440p display mode so the title picks a native 1440p output") describes (a), the secondary part. **Which is right:** our H1 stands for (a) (measured here, 8 runs); the Windows line's result is about (b)+(a) together and its mechanism is (b).
That (b) decides the output is: bytes checked on our image (the record holds 3840x2160, the table has the five modes; test), the title's own matching against that table (resolution.rs L3 doc: it matches its own list, not the DXGI one),
and the Windows line's runs of `0c1905bf` (not ours): so *probable, not proven here*. The two do not contradict. To separate them in one run: native 1440p with `VICTORY_DISPLAY_MODE=3840x2160` set by hand (`run.rs` keeps a value that is already set): a 2560x1440 source frame would show that reporting 1440p is not necessary with the image chain active. It would not isolate (b) from the other image edits. A different frame size calls for comparing the measured HLE mode and uncapped source sizes before attributing a cause.
`mac.res.native_1440p_switch` reconciles P2 with the Mac evidence: reporting a mode alone is not enough, and the perf-only image patch includes a likely source of the output size (`mac.loader.display_mode_record`). Its runtime effect remains unproven on Mac.

*Run plan* (not run here; the lead takes the game lock). Same method as the ui-scale runs 2026-11-28: Metal back, no keys, no input replay, present 320 (stable from 288), hint bar "choose account (Y) / PLAYER (A)":
`tools/mac/run.sh --perf --backend metal --purpose "new-nodes: mac.res.output,mac.res.ui_scale,mac.loader.display_mode_record,mac.loader.scanout_reservation" --no-keys --input none --timeout 300` with
`VICTORY_METAL_DUMP_PRESENT=288,320@<out>/dump` in the environment (needs a stage with the dump; `purpose: corpus` also fits). Read: (1) stdout `output chain: native 1440p on (the perf profile's default)`,
`display 2560x1440 SDR, scene 2560x1440`, `non-fidelity: output patched to 2560x1440 (native 1440p, in the image)`, no `resolution: ... 0x224352b` line, no `int3` at +0x247af25; (2) frame size of `present-320-source.png` = 2560x1440
(`mac.res.output`); (3) the hint bar's bounding box in that frame: 604x54 means U1 (1.5x), 403x36 means U2 or the frontend viewport (1.0x), anything else is new; compare with 604x54 at 3840x2160 / 1280x720 of the ui-scale runs.
Second run, replace `--perf` with `--fidelity --present-max none` (otherwise P1 caps the source handed to the Metal stage), with `VICTORY_NATIVE_1440P=1` in the environment: stdout carries `native 1440p stays off ... ignored`, source frame 3840x2160, hint bar 604x54 at (3156,2047) as run C.
Third run (control for the paragraph above): the first run again with `VICTORY_DISPLAY_MODE=3840x2160` in the environment. Add `--log-render-size` once to see the internal size (expected 2560x1440 at scale 1.0; 1707x960 with `VICTORY_RENDER_SCALE=0.667`).

Matrix: `mac.res.output`, `mac.res.internal`, `mac.res.drs`, `mac.res.display_mode`, `mac.res.ui_layout`, `mac.loader.resolution` (updated); new: `mac.loader.output_chain`, `mac.loader.display_mode_record`,
`mac.loader.scanout_reservation`, `mac.loader.temporal_scale_min`, `mac.loader.temporal_scale_controller`, `mac.loader.settings_preview_viewport`, `mac.loader.title_frame_rate_loads`, `mac.gpu.native_frontend_viewport`, `mac.res.ui_scale` (gap).
*Retire when:* the Windows line is merged into mac/livetr (the chain's commits then come with it; the profile rule, `output_chain.rs`, stays ours).
