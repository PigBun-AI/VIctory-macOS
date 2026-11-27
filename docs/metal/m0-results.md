# Metal backend, milestone M0: results

Route S keeps Wine and puts a Metal backend behind it: a PE `d3d12.dll`
calls, through a Wine unixlib, an x86_64 Mach-O that talks to Metal. M0 asks
whether that is possible at all. Date of the measurements and of every
version check below: **2026-11-27** (Asia/Shanghai).

**Verdict: Route S is feasible.** Details and the reasons are at the end.

Raw output lives outside the repository, in
`~/Documents/GTA6Project/research/probe-results/m0-*.txt` (listed at the end).
The probe is `metal/probe`, the bridge `metal/bridge`, the Metal side
`metal/back`; `tools/metal/build-unixlib.sh` stages them and
`tools/metal/run-m0.sh` runs them. Nothing outside `metal/`, `tools/metal/`
and `docs/metal/` was touched.

## 1. What was used (versions, checked 2026-11-27)

| Item | Version | How it was checked |
|---|---|---|
| macOS | 27.2.1 (26B8132) | `sw_vers` |
| Xcode / SDK | Xcode 27.2 (27B52), MacOSX SDK 27.2 | `xcodebuild -version`, `xcrun --show-sdk-version` |
| Machine | Apple M5 Max, unified memory | `sysctl`, Metal device name |
| Wine | `wine-11.0`, built from CrossOver sources **26.3.0** by `tools/macos/build-crossover-wine.sh`; x86_64, run under Rosetta | `wine --version`, `CROSSOVER_VERSION` default in the script |
| Rust | 1.98.1 (from the repository's `rust-toolchain.toml`, inherited by `metal/`) | `rustc --version`, `rustup show` |
| `objc2-metal` | 0.3.2 (latest on crates.io on 2026-11-27) | `metal/Cargo.lock`, crates.io API |
| `objc2` | 0.6.5 (latest) | same |
| `objc2-foundation`, `objc2-quartz-core`, `block2` | 0.3.2, 0.3.2, 0.6.2 | `metal/Cargo.lock` |
| `windows-sys` | 0.61.2 (latest) | same |
| `libc` | 0.2.191 (latest) | same |
| Upstream base | `origin/main` a509c3a | `git fetch origin` |

`objc2-metal` 0.3.2 already has the Metal 4 protocols (`MTL4CommandQueue`,
`MTL4CommandAllocator`, `MTL4CommandBuffer`) and `MTLResidencySet`; no raw
`msg_send!` was needed for them.

## 2. Measurements

Each Wine and native figure is the median of the per-process medians of three
separate processes; the range is over those three processes. Inside a process
the cheap calls are batches of 1,000,000 (or 500,000) calls, 21 batches; the
command-buffer figures are 4,200 samples; the bandwidth figures are 21 rounds
of a 64 MiB read after 30 warm-up rounds, with the sources taking turns
round by round so that a moving GPU clock affects all alike. The same suite
(`metal/probe/src/suite.rs`) runs three ways:

- **Wine**: `m0-probe.exe` (PE, `x86_64-pc-windows-gnullvm`) under Wine on
  Rosetta, calling `victory_metal_bridge.dll` -> `victory_metal_bridge.so`.
- **Rosetta native**: x86_64 Mach-O, the same code called in process (no Wine).
- **arm64 native**: the same, arm64.

`#221` is tebieshi's probe on an M2 Max (CrossOver Wine 26.3, Rosetta; the
ranges are theirs over three processes; issue #221, comment 5), quoted from
`research/metal-backend-plan.md` 2.2.

### Call cost (caller to unixlib and back)

| Metric | Wine, M5 Max (range) | #221 (M2 Max) | Rosetta native | arm64 native |
|---|---|---|---|---|
| Empty call | **25.6 ns** (25.5-25.8) | 32.9-33.1 ns | 1.2 ns | 0.7 ns |
| Call passing a 1 KiB pointer, untouched | 25.8 ns (25.7-25.9) | n/a | 1.2 ns | 0.7 ns |
| 256 x u32 written by caller, read by callee | **54.5 ns** (54.4-54.6) | 76.7-77.2 ns | 28.7 ns | 77.1 ns |
| the caller's 256-word fill alone, no call | 12.5 ns | n/a | 11.5 ns | 12.6 ns |

The Wine call cost is about 25 ns: Wine 54.5 ns minus Rosetta-native 28.7 ns
is 25.8 ns, the same as the empty call. After subtracting the caller's fill
(12.5 ns), a 256-word call costs about 42 ns. Whether #221's 77 ns includes
a fill is not stated. The native empty-call figures are an in-process function
call and only show the baseline; they are not a bridge cost. The arm64 256-word
figure (77 ns) is higher than Rosetta's (28.7 ns); I did not investigate it
(it is a native baseline and does not affect the bridge).

### Empty command buffer, commit and wait

| Metric | Wine | Rosetta native | arm64 native | #221 |
|---|---|---|---|---|
| Classic `MTLCommandQueue`, one unixlib call each, timed by the caller | **20.9 us** (20.9-21.0) | 19.9 us | 15.0 us | 21.8-22.2 us |
| Classic, loop of 200 inside one unixlib call | 21.6 us | 21.4 us | 14.5 us | n/a |
| `MTL4CommandQueue` (allocator, command buffer, commit, shared-event wait), one call each | **20.7 us** (20.7-20.9) | 19.9 us | 15.0 us | n/a |
| `MTL4CommandQueue`, loop of 200 inside one call | 21.2 us | 21.0 us | 14.9 us | n/a |

`p99` is about 35-43 us and the max a few hundred us (see the raw files).

Metal 4 works from an x86_64 Mach-O under Rosetta, and inside Wine: the
device reports the Metal 4 GPU family, and the queue, allocator, command
buffer and event were all created. Its empty round trip costs the same as the
classic queue (within 0.3 us). The per-call cost over a loop inside one call is
under 1 us, so most of 20 us is Metal's own commit-and-wait latency, not the
bridge.

### No-copy memory

All of these were run with 16 MiB regions and, under Wine, checked in both
directions (CPU writes, GPU sums; GPU writes, CPU compares every word; CPU
writes again, GPU sums again):

| Check | Result |
|---|---|
| `VirtualAlloc` is 16 KiB aligned (0xbe0000) | yes |
| `newBufferWithBytesNoCopy` accepts `VirtualAlloc` memory | yes (16 MiB wrapped in 60 us) |
| ... CPU->GPU, GPU->CPU, CPU->GPU again | all correct (4,194,304 words) |
| `MapViewOfFile3` view (pagefile-backed section) wrapped | yes (16 MiB in 17 us), same three checks correct |
| Wrap at +4 KiB (`victory_formats::pagetable::BLOCK`) inside a region | accepted (also in #221's probe) |
| Metal's own shared buffer | the baseline for the bandwidth below |

Reads of 64 MiB by a GPU kernel (GB/s, median over 21 rounds of the three
processes; GPU time = start to end of the kernel, wall = commit to completion
as seen by the CPU):

| Source | Wine, GPU time | Wine, wall | #221 (M2 Max) |
|---|---|---|---|
| `VirtualAlloc` | **561.8** (513-563) | 254.8 | 309-342 |
| `MapViewOfFile3` | **549.3** (517-562) | 259.7 | 313-328 |
| `VirtualAlloc` + `MTLResidencySet` | 560.8 (497-565) | 254.0 | n/a |
| Metal shared buffer | 557.9 (542-562) | 256.5 | n/a |

The wrapped memory is as fast as a buffer Metal made itself (within the
noise between processes); the M5 Max bandwidth is higher than the M2 Max's,
so the absolute numbers are not comparable to #221's. The native runs show the
same equality (arm64 mmap 528, shared 550, wrapped+set 556 GB/s; Rosetta 488,
498, 496).

### Residency set

The wrapped `VirtualAlloc` buffer was added to an `MTLResidencySet`
(create 53 us, add 1.5 us, commit 6.8 us, request residency 4.4 us, from one
run), the set reports it as contained (allocation count 1), a classic
command queue with the set attached still computes the right GPU sum, and a
Metal 4 queue with the set attached completes an empty command buffer. Read
bandwidth with the set is unchanged (above). Conclusion: wrapped Wine memory
can be made resident through a residency set; what residency changes for the
game's page-fault behaviour is M4's question, not measured here.

## 3. The three open points

### (1) Can `metal/` be a nested workspace that depends on `../crates/*` and inherits the toolchain? **Yes.**

- `metal/Cargo.toml` is its own `[workspace]`. `victory-metal-probe` has
  `victory-formats = { path = "../../crates/victory-formats" }`; it builds,
  and its test (`victory_formats::pagetable::BLOCK == 4096`) passes. The
  dependency's `version.workspace = true` fields resolved against the root
  workspace.
- The root `Cargo.toml` and `Cargo.lock` are untouched (`git status` lists
  only `metal/` and `tools/metal/`). `metal/Cargo.lock` is its own file.
- `rust-toolchain.toml` of the root applies (`rustup show` inside `metal/`:
  1.98.1, "overridden by .../rust-toolchain.toml").
- The root `.cargo/config.toml` is read from `metal/` too, so the
  `x86_64-pc-windows-gnullvm` linker (`tools/macos/gnullvm-cc.sh`) is used
  without any local config.
- `rustfmt.toml` of the root also applies (`rustfmt --print-config` in `metal/`
  gives `use_field_init_shorthand = true`, a setting from that file).
- Note: the toolchain file lists only the `x86_64-pc-windows-msvc` target;
  `x86_64-pc-windows-gnullvm` and `x86_64-apple-darwin` must be added once
  (`rustup target add`), as the main project's setup already does for gnullvm.

### (2) Does a later `WINEDLLOVERRIDES` entry override `d3d12,d3d12core,dxgi=b` of `victory.sh`? **Yes, the last entry wins.**

`tools/metal/test-override-order.sh` takes the exact assignment line from the
current `tools/macos/victory.sh`
(`WINEDLLOVERRIDES="mscoree,mshtml=;winedbg.exe=d;d3d12,d3d12core,dxgi=b${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"`),
puts a stand-in dll (an unmarked copy of the bridge dll) in the scratch
prefix as `system32/d3d12.dll` and `dxgi.dll`, and asks `m0-probe.exe` to
load `d3d12.dll` and tell which one answered (6 of 6 cases as expected):

| Case | Result |
|---|---|
| `victory.sh` alone (`=b`) | Wine builtin loaded |
| user value `d3d12,dxgi=n,b` appended after `=b` | **our dll loaded** |
| user value `d3d12,dxgi=n` appended | our dll loaded |
| reverse order (`=n,b` first, `=b` last) | builtin: the last one wins |
| `LoadLibraryExW(... LOAD_LIBRARY_SEARCH_SYSTEM32)`, as `root.rs` does, with `=b` only | builtin |
| the same, user value `d3d12,dxgi=n,b` appended | **our dll loaded** |

So `tools/macos/victory.sh` does not need a change: `run.sh` of M1 can
export `WINEDLLOVERRIDES="d3d12,dxgi=n,b"` before it. (The scratch prefix
`research/m0-prefix` is not the game's; the script puts back what it
replaced. A stand-in in `system32` also works with the system32-only search.)

### (3) Can our own unixlib get winemac.drv's `CAMetalLayer` for a window, and draw into it? **Yes.**

`victory_metal_bridge` finds the already loaded `winemac.so` (dyld image
list + `dlopen(RTLD_NOLOAD)`), takes its exported `macdrv_functions` table,
and calls the entries in the order D3DMetal does (`get_win_data`,
`create_metal_device`, `view_create_metal_view`, `view_get_metal_layer`,
`release_win_data`). It uses only the slot order in `d3dmetal.c`; the slot
indices are constants in `metal/bridge/src/unix.rs` and need re-checking when
the Wine build changes (the stage reached is reported, 0 to 5, to show where
a mismatch stops).

Under Wine, `m0-probe.exe --layer` creates a window (`CreateWindowExW`) and
passes its HWND to the unixlib. In all three full runs: stage 5 reached, a
`CAMetalLayer` and a view returned, a 632x366 drawable (pixel format 80,
BGRA8) obtained, cleared with (0, 0.5, 1, 1), committed and presented, and the
top-left pixel read back as 0xff0080ff, the expected colour.

Limits: the colour was verified by reading the drawable texture back, not
by looking at the screen (my screenshot attempt showed an unrelated
foreground window; I deleted it rather than keep it). The layer was acquired
on the thread of the unixlib call, not the main thread, and it worked; resize,
`CAMetalLayer` properties (display sync, EDR) and teardown with the window
were not exercised. M1's first present test should include a visible check.

## 4. Comparison with the #221 reference values

| Quantity | #221 (M2 Max) | Here (M5 Max) | Reading |
|---|---|---|---|
| PE -> unixlib empty call | 32.9-33.1 ns | 25.6 ns | same order, a bit lower; within the 30-80 ns the plan assumed |
| 256-word call | 76.7-77.2 ns | 54.5 ns (42 ns net of the fill) | lower |
| Empty command buffer | 21.8-22.2 us | 20.9 us | same |
| 64 MiB `VirtualAlloc` GPU read | 309-342 GB/s | 549-562 GB/s | chip difference; wrapped = Metal-owned |
| 64 MiB `MapViewOfFile3` GPU read | 313-328 GB/s | 517-562 GB/s | same |
| Stock Wine needs no rebuild | yes ("Wine-marked PE anchor" + PE/Mach-O pair under `WINEDLLPATH`) | yes, same layout | reproduced |

How the unixlib is set up (no C, no winegcc, no Wine rebuild): the PE half
is a Rust cdylib whose DLL is marked as a Wine builtin by `winebuild
--builtin`; Wine then looks for `x86_64-unix/victory_metal_bridge.so` next to
it in `WINEDLLPATH`. The Mach-O half is the same crate built for
`x86_64-apple-darwin`, exporting `__wine_unix_call_funcs`. The PE side loads
the dll, asks ntdll for its unixlib handle (`NtQueryVirtualMemory`,
`MemoryWineUnixFuncs`) and calls `__wine_unix_call_dispatcher` (`pe.rs`).
`tools/metal/build-unixlib.sh` checks the mark, the export and the architecture.

## 5. Checks of the skeleton

- `metal/`: `cargo test` (aarch64) 8 tests pass (`bridge` 5, `probe` 2 + 1
  integration test that runs the whole suite in process with small sizes);
  the same under `--target x86_64-apple-darwin` (Rosetta). `cargo clippy
  --all-targets -- -D warnings` is clean for aarch64-apple-darwin,
  x86_64-apple-darwin and x86_64-pc-windows-gnullvm; `cargo fmt --check` is
  clean.
- `tools/metal/check-layers.sh` (layering rule 3 and the related dependency
  rules) passes on the skeleton, and `--self-test` shows each rule catches a
  violation (22 cases).
- Root `cargo test --workspace --locked` (native macOS): does **not**
  compile on a pristine `origin/main` a509c3a either: `victory-shader`'s
  lib test refers to modules gated with `#[cfg(all(test, windows))]` (48
  errors). This is independent of `metal/`, and the result is identical in the
  worktree and in a clean checkout of `origin/main`. With
  `--exclude victory-shader`: 459 passed, 0 failed, 15 ignored, in both. So
  the Metal skeleton does not change the root workspace.

## 6. Problems met

- No GPU or Wine blocker. Custom unixlibs are allowed by stock Wine
  (CrossOver 26.3.0 build), as #221 reported.
- The first bandwidth run measured sources one after the other, so the later
  ones saw a warmer GPU; fixed by interleaving rounds and dropping 30
  warm-up rounds (all numbers above come from the fixed version).
- `victory-shader` tests do not build natively (item above); upstream issue,
  not touched.
- Screenshot of the layer window not kept (see 3 (3)).

## 7. Judgment: Route S is feasible

1. A PE and a Rust unixlib pair under stock Wine on Rosetta reaches x86_64
   Metal: 25 ns per call, 21 us per empty command buffer.
2. Memory the title gets from `VirtualAlloc` and `MapViewOfFile3` can be
   wrapped without copy, is correct both ways and as fast as Metal's own
   buffers; a residency set works over it, with classic and Metal 4 queues.
3. The three open points of the architecture note are closed: a nested
   workspace works with zero upstream change; `victory.sh` does not need a
   hook; the `CAMetalLayer` of a Wine window is reachable from the unixlib.

There was no case that would send the work to Route B.

### Effect on M1

- No change of plan. M1 can use the layout of `tools/metal/build-unixlib.sh`
  as is, and `run.sh` needs only `WINEDLLOVERRIDES="d3d12,dxgi=n,b"` and the
  files in the prefix's `system32`.
- Call cost is small: buffering commands in the front end and one `submit` per
  `ExecuteCommandLists` is still the right default, but a per-call chatty
  interface would also cost only about 26 ns per call; the 20 us of a
  submission dominates.
- Metal 4 queue and the classic queue cost the same for an empty submission;
  the choice between them for M1 can be made on API fit, not speed (not
  measured with real work).
- Not covered by M0 and still open: Metal Shader Converter (compute,
  mesh, bindless), the same `macdrv_functions` slot order on other Wine
  builds, present details beyond a clear, and a visible screen check.
- To reproduce:
  `tools/metal/build-unixlib.sh && tools/metal/run-m0.sh --layer`,
  `tools/metal/test-override-order.sh`, `tools/metal/check-layers.sh`.
  They need the Rust targets `x86_64-pc-windows-gnullvm` and
  `x86_64-apple-darwin`, and the Wine in `target/wine-cx` of the main
  checkout (`VICTORY_WINE_DIR` overrides).

## 8. Raw outputs

`~/Documents/GTA6Project/research/probe-results/`: `m0-wine-run{1,2,3}.txt`,
`m0-native-arm64-run{1,2,3}.txt`, `m0-native-x86_64-rosetta-run{1,2,3}.txt`,
`m0-summary.txt` (medians across processes), `m0-override-order.txt`,
`m0-check-layers.txt`, `m0-env.txt` (versions),
`m0-root-cargo-test*.txt` (the root test runs: `m0-root-cargo-test.txt` and
`m0-root-cargo-test-clean-origin-main.txt` are the failing full runs,
`m0-root-cargo-test-excl-shader-*.txt` the runs with `--exclude victory-shader`).
