# Running the title on a Mac: `tools/mac/run.sh`

One script starts `victory.exe` on this Mac, on either backend, with the same configuration for both. It is not a build step:
`tools/mac/check.sh` builds (`target/x86_64-pc-windows-gnullvm/release/victory.exe`). Patch notes: `docs/mac/PATCHES.md` (M4).

```sh
tools/mac/run.sh [options] [-- arguments for the title]
tools/mac/run.sh --print-env [options]        # print what would be passed and the command; start nothing, change nothing
```

Nothing is written outside this checkout's `target/` and the data directories named below; the game directory is only read.

## The two choices

| Choice | Option (variable) | Values | Default |
|---|---|---|---|
| Backend | `--backend` (`VICTORY_MAC_BACKEND`) | `d3dmetal`: Apple's D3DMetal in CrossOver Wine. `metal`: our Metal backend (`d3d12.dll`, `dxgi.dll` and the bridge in the prefix) | `d3dmetal` |
| Profile | `--profile`, `--fidelity`, `--perf` (`VICTORY_MAC_PROFILE`) | `fidelity`, `perf` | `perf` (until the owner switches it, `PATCHES.md`) |

**The backend decides nothing but:** the prefix, the dlls in its `system32`, `WINEDLLPATH`, `WINEDLLOVERRIDES`, `VICTORY_METAL_LOG` and
`VICTORY_METAL_NOTIMPL_LOG`. Everything else below is set the same way for both. `--print-env --backend d3dmetal` and `--backend metal`
diff to those lines, and the `#` comment lines that name the backend and prefix. **Allowed differences between the backends** (the
only ones): the backend choice, `WINEDLLPATH` / `WINEDLLOVERRIDES`, `VICTORY_METAL_LOG` / `VICTORY_METAL_NOTIMPL_LOG`, the prefix path
(`WINEPREFIX`), and the data and cache directories (`VICTORY_DATA_DIR`, `VICTORY_CACHE_DIR`, `VICTORY_DXC_DIR`; per backend, same seed).

### Profiles

- **fidelity**: Xbox Series X (no `--console`), the title's output size comes from its own settings, 2560x1440; the output is not
  patched (`VICTORY_OUTPUT=0`); internal resolution, dynamic resolution and TAA upscaling are the title's own logic. Before the run
  `video/resolutionFullscreen` in the *data directory's* `settings.xml` is set to `--settings-output` (default `2560x1440`, one of the
  title's modes); the original is kept once as `settings.xml.before-fidelity`. The profile has its own data directory
  (see "Data directories" below), so the other runs' saves and settings are not touched. Refused in this profile: `--console series-s`, `VICTORY_RENDER_SCALE`, `VICTORY_UPSCALER`, a `VICTORY_OUTPUT` that is not
  `0`/`none`. **Pool check rule:** the profile does *not* skip the back buffer pool check by default. If a real F0 run stops at it
  (title+0x247af25, `int3`), `--pool-check skip` becomes a fixed part of the fidelity profile; the log then carries a
  `non-fidelity: pool check skipped` line (to be added to the loader then), and the node is registered in the matrix as a **stubbed**
  node (a title patch, not the title's own behaviour). Until then it is only an option. Details: `PATCHES.md`, L3.
- **perf**: `VICTORY_OUTPUT=1280x720` (an own `VICTORY_OUTPUT` in the environment wins), the patch L3; the run log carries
  `non-fidelity: output patched to 1280x720`. For speed and debugging; no fidelity result may cite it. `--console series-s` is allowed.

### Backends

- **d3dmetal**: prefix `target/wineprefix-livetr-d3dmetal` (cloned from the old `target/wineprefix-livetr` on first use if that exists,
  else made by Wine). If the prefix's `system32` holds `d3d12.dll.metal-orig` / `dxgi.dll.metal-orig` they are put back first.
- **metal**: prefix `target/wineprefix-livetr-metal`, made with `wineboot` on first use. The stage is `metal/target/wine-stage`
  (`VICTORY_METAL_OUT` changes it); `front/d3d12.dll`, `front/dxgi.dll` and `x86_64-windows/victory_metal_bridge.dll` are copied into the
  prefix's `system32`, the originals first kept as `*.metal-orig` (once). No stage: the script says to run `tools/metal/build-unixlib.sh`
  (`--build` runs it). `WINEDLLPATH=<stage>[:...]`, `WINEDLLOVERRIDES` ends in `d3d12,dxgi=n,b`, `VICTORY_METAL_LOG=1`,
  `VICTORY_METAL_NOTIMPL_LOG=<Wine Z: path of the run's out directory>\notimpl.log`. The D3DMetal prefixes (`wineprefix-play`,
  `wineprefix-livetr`, `wineprefix-livetr-d3dmetal`) are refused. Debug switches of the back (`VICTORY_METAL_TRACE`, `_STEP`,
  `_SKIP_GS_INDIRECT`, `_DROPSTATS`, `_CALLSTATS`, `_ARCHIVE`, `_LENIENT`) are only passed through from the environment, never set;
  skipping geometry-shader indirect draws is not a default.

## Data directories

Each profile and backend has its own: `<parent>/victory-data-livetr-<profile>-<backend>/{data,cache,dxc}` (for example
`victory-data-livetr-fidelity-metal`). On first use it is cloned (APFS `cp -c`, no extra space until written) from one **seed**
directory, `victory-data-livetr` (`--seed`). The seed is only read, never written, by `run.sh`; so two backends start from the same
state every time and can run at the same time (a second `victory.exe` still needs `--allow-concurrent`, and two games share the GPU).
`--fresh-data` replaces the run's directory with a new clone first (reproducible start; it also drops the translated-shader cache, so the
run is a cold one). The fidelity profile then sets `video/resolutionFullscreen` in its clone's `settings.xml` (original kept once as
`settings.xml.before-fidelity`). `--data`, `--cache` or `VICTORY_DATA_DIR` use that directory as it is (no cloning, no `--fresh-data`).
Do not use a seed another game is writing at that moment if you need an exact state: the clone is taken from the files as they are.

## Options

| Option | Effect | Default |
|---|---|---|
| `--exe PATH` | the `victory.exe` to run | `target/x86_64-pc-windows-gnullvm/release/victory.exe` |
| `--out DIR` | logs go here | `target/mac-runs/<time>` |
| `--timeout SEC` | stop the run after SEC seconds (0: no limit) | 900 |
| `--data DIR`, `--cache DIR` | the title's saves and the derived cache (`VICTORY_DATA_DIR` in the environment also works) | `victory-data-livetr-<profile>-<backend>/{data,cache}` next to the checkout (below) |
| `--seed DIR` | the read-only seed the data directories are cloned from | `victory-data-livetr` next to the checkout |
| `--fresh-data` | delete the run's own data directory and clone it from the seed again, so the run starts from the seed state (only with the default directory) | off |
| `--prefix DIR` | the Wine prefix (`WINEPREFIX` also works) | by backend, see above |
| `--keys SPEC`, `--no-keys` | `VICTORY_KEYS` | `8-3600/3:Enter` (Enter every 3 s from 8 s; patch I1) |
| `--input FILE\|none` | `VICTORY_INPUT_REPLAY` | `tools/mac/input/world-neutral.txt` (neutral input from world frame 1; I1) |
| `--record FILE` | `VICTORY_INPUT_RECORD` (relative to the out directory); turns the default replay off | off |
| `--capture-frames LIST` | `VICTORY_CAPTURE_FRAMES` (diagnostics builds): comma list of present numbers and `w<N>` (I2, see "Capture runs"), e.g. `64,w12`; an item that is neither is refused | off |
| `--record-at f<N>\|w<N>\|SEC` | `VICTORY_RECORD_AT` (diagnostics builds): when the frame recorder starts. `f<N>` present number, `w<N>` the N-th world frame after world frame 1 (I2), `SEC` seconds. Records only with `VICTORY_RECORD=<folder>` in the environment (the script warns if it is missing) | off |
| `--no-watchdog` | `VICTORY_MAC_NO_WATCHDOG=1` (L5): the title's own hang watchdog off. **Non-fidelity**, logged (see "Capture runs") | off |
| `--corpus` | preset for capture-corpus runs: `--record-at <--corpus-frame>` plus `--no-watchdog`. An explicit `--record-at` wins over the preset's. **Non-fidelity** | off |
| `--corpus-frame w<N>` | the frame `--corpus` records at (a bare `N` means `w<N>`) | `w5` |
| `--frametime-log` | `VICTORY_FRAMETIME_LOG=<out>/frametime.log` (frame, microseconds since the previous sample, draws) | off |
| `--tracking 0\|1` | `VICTORY_FAULT_TRACKING` | 1 |
| `--masked SPEC` | `VICTORY_PATCH_MASKED` | not set (follows tracking; the extra #144 sites on build 1.0.2.1) |
| `--present-max WxH\|none` | `VICTORY_PRESENT_MAX`: host window and swap chain only (P1) | not set |
| `--pool-check skip\|keep` | `VICTORY_POOL_CHECK` (L3) | not set (kept) |
| `--log-render-size` | `VICTORY_LOG_RENDER_SIZE=1` (P2): not for timing runs | off |
| `--settings-output WxH` | fidelity only: what is written into `settings.xml` | `2560x1440` |
| `--console series-x\|series-s` | the console model the title sees; `series-x` passes no `--console` | `series-x` |
| `--build` | metal: run `tools/metal/build-unixlib.sh` first | off |
| `--sample SEC` | every SEC seconds a line in `<out>/mem.csv`: GPU memory in use and allocated (`ioreg`), GPU `recoveryCount`, swap, `vm_stat` pages, RSS of all `victory.exe` | off |
| `--allow-concurrent` | start although another `victory.exe` runs (two games at once ran a 32 GB Mac out of GPU memory) | refused |
| `--print-env`, `--dry-run` | print and exit | |
| `--exec` | run `victory.exe ARGS` (the arguments after `--`, e.g. `-- warm --jobs 4`) in the foreground instead of the title, with the environment, prefix, DLLs, data and cache of a run; no keys, no time limit, no log folder, exit status is `victory.exe`'s | off |
| `-- ARGS` | passed to `victory.exe run --execute <game>` after `--console` (with `--exec`: the whole `victory.exe` command line) | |

Variables read: `GTA6_ROOT` (the parent of the checkouts and data, default: the checkout's parent), `GAME_DIR` / `VICTORY_GAME_DIR` (the game
directory, default `GTA6.Xbox.v1.0.2.1` in `GTA6_ROOT`), `VICTORY_MAC_WINE_DIR` (Wine, default `VIctory-macOS/target/wine-cx`), `WINEDEBUG`
(default `-all`). Every other `VICTORY_*` / `WINE*` variable passes through unchanged and is listed by `--print-env`.

## What it sets (both backends)

`WINEPREFIX`, `WINEDEBUG`, `WINEDLLOVERRIDES`, `ROSETTA_ADVERTISE_AVX=1`, `VICTORY_SKIP_CPU_CHECK=1`; `VICTORY_GAME_DIR`, `VICTORY_DATA_DIR`,
`VICTORY_CACHE_DIR`; the helper shaders' DXC (G4): `VICTORY_DXC_HELPER` (built once from `tools/macos/dxc-d3dmetal.sh` into `target/`),
`VICTORY_DXC_DIR` (`dxc` next to the cache), `DXC_LIB` (D3DMetal's `libdxcompiler.dylib` in the Wine tree); `VICTORY_FAULT_TRACKING`;
`VICTORY_KEYS`, `VICTORY_INPUT_REPLAY` (or neither, with `--no-keys` / `--input none`); `VICTORY_OUTPUT` (by profile); and, when asked
for, `VICTORY_PRESENT_MAX`, `VICTORY_POOL_CHECK`, `VICTORY_LOG_RENDER_SIZE`, `VICTORY_FRAMETIME_LOG`, `VICTORY_CAPTURE_FRAMES`,
`VICTORY_RECORD_AT`, `VICTORY_MAC_NO_WATCHDOG`, `VICTORY_INPUT_RECORD`, `VICTORY_PATCH_MASKED`. `--print-env` lists them all. Only the wineserver of the run's own prefix is ever stopped.

## Capture runs: `--record-at`, `--capture-frames`, `--no-watchdog`, `--corpus`

**`w<N>`** (patch I2) names the N-th world frame after world frame 1 (`w0` is world frame 1, `w300` world frame 301). A present number
(`f300`) names a different moment in every run, `w300` the same one. `--capture-frames` takes it next to plain present numbers
(`--capture-frames 64,w12`); `--record-at` takes `f<N>`, `w<N>` or seconds. `w0` cannot trace the first world frame's draws ahead of time
(use `w1` or later), and when the world is never reached, nothing triggers. Recording also needs `VICTORY_RECORD=<folder>` in the
environment and a diagnostics build (`docs/replay.md`); `run.sh` does not set the folder.

**`--no-watchdog`** (patch L5) sets `VICTORY_MAC_NO_WATCHDOG=1`: the title's own hang watchdog, which kills the title in the world when a
frame takes too long, is turned off (three jumps patched at load, build 1.0.2.1 only; otherwise it stays on and the log says why). The run is
**non-fidelity**: the log carries `non-fidelity: title hang watchdog disabled`, and `--print-env`, `run-env.txt` and `start.txt` say so; no
fidelity result may cite it. **Use it with a time limit.** With the watchdog off, a run that really hangs no longer crashes: nothing ends it
but `--timeout SEC` (default 900 s, which stays; `--timeout 0` plus `--no-watchdog` makes the script warn). Check `summary.txt` for
`time limit ... reached` in `stderr.log` to tell a hang that the limit ended from a run that finished by itself.

**`--corpus`** is the preset for filling the capture corpus: `--record-at w<N>` plus `--no-watchdog`, with `N` from `--corpus-frame`
(default **`w5`**). The default is low on purpose: with D3DMetal the game usually crashes within about 10 frames of entering the world, so
a later frame is often never reached, while the recorder arms at `w5` and takes the next whole frame (a few frames later, as with `f<N>`).
A slower recording frame needs the watchdog off, which is why the preset includes it. Same options on both backends:

```sh
VICTORY_RECORD=$HOME/captures tools/mac/run.sh --corpus --backend d3dmetal --timeout 600
tools/mac/run.sh --print-env --corpus --backend metal        # shows VICTORY_RECORD_AT=w5 and VICTORY_MAC_NO_WATCHDOG=1
```

## What a run leaves in its out directory

`stdout.log`, `stderr.log` (the title and Wine), `start.txt` (time, backend, profile, checkout commit and number of changed files, exe
hash and time, the `settings.xml` size or the non-fidelity line), `run-env.txt` (the `--print-env` text of this run), `summary.txt`
(status, key log lines, last `ms/frame` lines, a count of GPU error lines), `end.txt`, and as asked `mem.csv`, `frametime.log`,
`notimpl.log` (metal).

Comparing two setups: `diff <(tools/mac/run.sh --print-env --backend d3dmetal) <(tools/mac/run.sh --print-env --backend metal)`.

Plan for the first fidelity runs: `docs/mac/fidelity-experiment.md`.
