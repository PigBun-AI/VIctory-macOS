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
`VICTORY_KEYS` takes `<s>:<key>,...` or `<from>-<to>:<key>` (held); the `a-b/3:Enter` form of the Metal
branch is not parsed here (the entry is dropped silently), so `run.sh` writes one `<s>:Enter` per 3 s.

### Findings of the first run under Rosetta (2026-11-27, not fixed)

- `VICTORY_FAULT_TRACKING=1`: the process stops with `rosetta error: unexpectedly need to EmulateForward on a
  synchronous exception` at `victory_safe_merge_at` of **`safecopy.rs`** (our own `vpmaskmovd` store into a
  title page the tracker guards, in `land.rs`' merge). The loader's masked-store patch covers the title's code, not
  this asm; `victory_safe_put` has the same store. Default (tracking off under Rosetta) does not guard pages
  and does not reach it. A fix would be per-dword plain stores for `merge`/`put` when `stores_need_patching()`.
- `titlewrite.rs`, `shadow.rs`, `inplace.rs`: no fault or hang seen with tracking off (default).

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
