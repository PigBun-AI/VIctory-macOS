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

### G2 — fault tracking off under Wine on Rosetta (commit `7deccf01`)

*What.* New `victory-gpu/src/mac/rosetta.rs`: `rosetta()` (ntdll `wine_get_host_version` says
"Darwin"), `fault_tracking()` (default on, off under Rosetta, `VICTORY_FAULT_TRACKING=1/0`
overrides, always on in tests), `stores_need_patching()`, `wait_for_overlapped_reads()`.
Hooks: `watch.rs` `writes()` returns `None` when tracking is off (the one entry to `guard()` and
the kernel write watch, so every caller copies the memory whole), `xmem.rs` leaves out
`MEM_WRITE_WATCH` on the pools, `files.rs` `ReadFile` waits for an overlapped read when tracking
runs under Rosetta; `victory-hle` re-exports the module for the app.

*Why not needed on Windows.* Windows resumes a store that faults partway; Rosetta 2 cannot
("unexpectedly need to EmulateForward on a synchronous exception", process hangs; VIctory-macOS
#1, #14, #50, #144). On a PC `rosetta()` is false and every hook does what it did before.

*Caveat for review.* The Mac fork had a different tracker (`Tracked`); this line's journal
(`GetWriteWatch` plus guards in `watch.rs`) was hooked at `writes()`/`track()` by reading, because
all its `VirtualProtect` calls on views sit behind `guard()`, which only `writes()` calls. Other
users of title-memory protection (`titlewrite.rs`, `shadow.rs`, `inplace.rs`, `safecopy.rs`) were
read, not run. **First thing to check on a run under Rosetta.**

*Test.* `mac::rosetta::tests::the_variable_decides_and_tests_track` asserted `!rosetta()`, so it always failed
under Rosetta; it now asserts what follows from `rosetta()` (`stores_need_patching`, `wait_for_overlapped_reads`),
which holds on a PC, under Wine on a Mac and natively.

*Remove when.* A macOS/Rosetta release resumes such stores (retest with `VICTORY_FAULT_TRACKING=1`
and without the patches), or the Metal backend replaces the D3D12 path on the Mac.

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

### M4 — `tools/mac/run.sh`

Runs a `victory.exe` under CrossOver Wine + D3DMetal in this checkout's own prefix
(`target/wineprefix-livetr`: only that prefix's wineserver is ever stopped), outside-the-repo cache and data
(`$VICTORY_DATA_DIR`, default `victory-data-livetr/data` next to the checkout), scripted keys, logs in `target/mac-runs/<time>`. Keys: this line's
`VICTORY_KEYS` takes `<s>:<key>,...`, `<from>-<to>:<key>` (held) and, with patch I1, `<from>-<to>/<every>[+<hold>]:<key>`
(`run.sh` itself still writes one `<s>:Enter` per 3 s, which also runs an exe built before I1). `VICTORY_INPUT_RECORD` and `VICTORY_INPUT_REPLAY` (I1) pass through from the environment.

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

### L1 — masked 256-bit stores rewritten (commit `781672bc`, `victory-loader/src/mac/masked.rs`)

AVX masked stores (`vpmaskmovd/q`, `vmaskmovps/pd`, `vcvtps2ph` to memory) redone as plain stores
Rosetta can resume, whenever tracking runs under Rosetta (`VICTORY_PATCH_MASKED`: `all`, `none`
or hex RVAs). `apps/victory/src/mac.rs` adds the extra RVAs of build 1.0.2.1 (hash `b99f876b…`).
Not needed on Windows: it resumes them. Remove with G2.

### L2 — 256-bit stores split into two 128-bit stores (`mac/wide.rs`, `xtask/src/wide_stores.rs`)

`VICTORY_PATCH_WIDE=<list>` (exe) and `VICTORY_PATCH_WIDE_DIR` (title DLLs; hook in `windows.rs`);
the list comes from `cargo xtask wide-stores <cached image>` (a disassembler pass, new xtask
command). Rosetta aborts on a fault in the second page of a 32-byte store. Remove with G2.

### L3 — title output size 1280x720 by default on a Mac (`mac/resolution.rs`)

`VICTORY_OUTPUT`, `VICTORY_RENDER_SCALE`, `VICTORY_UPSCALER`; compares each site byte for byte
and leaves it alone if the build differs (build 1.0.2.1 only). The default 1280x720 applies
under Rosetta only. Not needed on Windows (the user picks the size in the title). Remove when
the Mac GPU path is fast enough for the title's own size.

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
