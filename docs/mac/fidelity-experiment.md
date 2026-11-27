# Fidelity profile: stability comparison (plan, not run)

Written 2026-11-27. Nothing here has been run: the branch `mac/fidelity-1440p` only prepared the code and scripts.
The default profile of `tools/mac/run.sh` is still **perf**; this plan runs **fidelity** next to it and decides nothing about
switching (that waits for the 3D-missing question).

## Question

With the title left to its own logic (Series X, output 2560x1440 from its settings, no resolution patch), does the title
1. get into the world and stay there (stability),
2. render at the internal size its own scale gives (expected 2/3 of 1440p = **1707x960**; the perf runs' 853x480 is 2/3 of 720p),
3. at what frame time and memory / GPU memory cost, and
4. without GPU hangs or resets,

compared with perf (output patched to 1280x720, internal 853x480, 4x fewer pixels at every stage)? Numbers from the earlier report
(PR #278: GPU 273 -> 234 ms at 3840x2160 -> 1280x720) are not a prediction for this machine; this plan measures it.

## Fixed for every run

- Same commit, clean tree (`git status` empty; `start.txt` records commit and changed-file count), same exe built by `tools/mac/check.sh`
  from it (`start.txt` records the exe hash), backend `d3dmetal` (Metal later, same plan with `--backend metal`).
- Nothing else uses the GPU (another `victory.exe` makes `run.sh` refuse; also close other GPU-heavy apps), mains power, same display
  setup, same time-limit, same keys and neutral input (the defaults), no `--present-max` unless the step says so.
- Save everything open first: a GPU reset can take down WindowServer (reports in #131), and a kernel panic was seen once on an M3 Max.
- Run order alternates (P F P F P F) so that thermal state and caches do not favour one side. The first run of each profile only
  warms the caches (`victory-data-livetr-<profile>-<backend>/cache`, translated shaders; `--fresh-data` makes a run cold again); only runs 2 and 3 count for timing.
- Each run: `tools/mac/run.sh --profile <p> --out target/mac-runs/<name> --timeout 900 --sample 5 --frametime-log` (and the
  step's extra flags). 15 minutes is the limit; stop earlier by hand if swap passes 20 GB or `gpu_recovery_count` rises (see below).

## Steps

| Step | Command (after `tools/mac/check.sh`) | Purpose |
|---|---|---|
| **C** calibration | `run.sh --perf --log-render-size --out target/mac-runs/calib` | The diagnostic's rule must give **853x480** with output 1280x720 (`render size: world frame N: internal 853x480 (scale 0.6664...`). If not, the rule is wrong and every later "internal" number is void. Also the first `world frame 1`. One run, no timing use. |
| **F0** fidelity as is | `run.sh --fidelity --log-render-size --out target/mac-runs/f0` | The expected stop: the back buffer pool check at title+0x247af25 (an `int3`; pool sized for 3840x2160). Look for a breakpoint exception at that address in `stderr.log`/`stdout.log`. If the world is reached, go to T. |
| **F1** pool check skipped | `run.sh --fidelity --pool-check skip --log-render-size --out target/mac-runs/f1` | Only if F0 stopped at the pool check. This is not a resolution patch (it changes no size) but it **is** a title patch; say so wherever its numbers are used. `resolution: back buffer pool's left-over check (skip)` and a `settings.xml ... 1280x720 -> 2560x1440` line must be in the log. |
| **T** timing, 3 pairs | alternate `run.sh --perf --out target/mac-runs/p<n> --frametime-log --sample 5` and `run.sh --fidelity [--pool-check skip as F decided] --out target/mac-runs/f<n> --frametime-log --sample 5` | Frame time, memory, hangs, without the render-size diagnostic (it takes a lock per draw). |
| **R** render size under load | one fidelity run with `--log-render-size`, 15 min, after T | Does the internal size stay 1707x960 for the whole stay, or does a `render size: changed:` line show up (dynamic resolution)? |
| **P** (optional) host window | `run.sh --fidelity [...] --present-max 1280x720 --out target/mac-runs/f-cap` | P1: the title keeps 2560x1440 / 1707x960 (check with `--log-render-size`), the host window and swap chain are 1280x720; log `VICTORY_PRESENT_MAX: the picture is drawn scaled...` and `presenting to a window` with 1280x720. First use of that code. |

Between runs `run.sh` stops only its own prefix's Wine; wait for `end.txt` before the next one.

## What to read in each out directory

| File | Line / column | For |
|---|---|---|
| `start.txt` | commit, changed files, exe hash, `settings.xml` size (fidelity) or `non-fidelity: output patched to 1280x720` | the run is what it says |
| `stdout.log` | `resolution:` lines (`Done`, or `skip` of the pool check); `non-fidelity:` (perf only, **must not appear in fidelity**); `render size:` lines | no resolution patch in fidelity; internal size |
| `stdout.log`/`stderr.log` | `presenting to a window width= height=` (expect 2560x1440 fidelity, 1280x720 perf); `display mode reported to the title`; `video memory as the system counts it` (before/after the swap chain, after a resize) | output, window, video memory budget |
| `stderr.log` | `world frame 1 (input scripts ...` | the world was entered (the replay is set, so the latch is logged) |
| both | `ms/frame` lines (the host's meter: fps, ms/frame, preparing, GPU, draws) | frame time cross-check |
| `frametime.log` | rows `frame, microseconds since the previous sample, draws` | frame time distribution |
| `mem.csv` | `gpu_in_use_bytes`, `gpu_alloc_bytes`, `gpu_recovery_count`, `swap_used`, `pages_*`, `victory_rss_kb` every 5 s | memory, GPU memory, resets |
| `summary.txt` | exit status, seconds, GPU error count | quick view |
| data dir `<title id>/crashes.log` | new entries | title crashes |
| macOS | `log show --last 20m --predicate 'eventMessage CONTAINS[c] "GPU" AND (eventMessage CONTAINS[c] "hang" OR eventMessage CONTAINS[c] "timeout" OR eventMessage CONTAINS[c] "reset")'` and `ls -t /Library/Logs/DiagnosticReports | head` | system-side GPU resets and panics (adjust the predicate to what the log really says) |

## Metrics and how to compute them

1. **Gets into the world.** `world frame 1` present (yes/no) and the time of it (`stderr.log` timestamp minus `start.txt`);
   the number of world frames reached (last `frame` of `frametime.log` minus the present number at `world frame 1`). Stable = in the
   world for the whole limit (900 s minus the load time) with no exit before the limit (`end.txt`: exit 143 = our time limit, anything
   else is a stop; check `crashes.log`).
2. **Internal size.** From `render size:` (C, F0/F1, R): the verdict `internal WxH`, its scale to the output, `confirmed=`, and every
   `changed:` line. Expected: perf 853x480 (0.6664), fidelity 1707x960 (0.6668). A different scale or a `changed:` line in a steady
   scene = the title does scale dynamically (then say between which sizes and when).
3. **Frame time.** From `frametime.log` rows after world frame 1 + 100 (skip the load): median and p95 of microseconds, draws/frame
   median; same from the `ms/frame` lines (GPU and preparing parts). Report perf and fidelity as pairs and the ratio. Not the first
   run of each profile.
4. **Memory.** From `mem.csv` after world frame 1: max and final `gpu_in_use_bytes` / `gpu_alloc_bytes`, max `swap_used`, min `pages_free`,
   max `victory_rss_kb`; from the log the `video memory` budget lines before and after the swap chain (difference = the cost of the
   back buffers). Difference fidelity - perf per run pair.
5. **GPU hangs.** `gpu_recovery_count` last minus first (must be 0); lines in the logs with `kIOGPUCommandBufferCallbackError`, `Hang`,
   `SubmissionsIgnored`, `queue replaced`, `device removed`; the system log query above; any `DiagnosticReports` file from the run.

## Decision rules for the report (suggested)

- **Fidelity is a usable baseline** if it enters the world in 2 of 3 timing runs, no GPU reset (`gpu_recovery_count` unchanged), no
  swap growth beyond the perf runs' by more than the cost of the larger targets, and C confirmed the diagnostic.
- If F0 stops at the pool check and F1 then runs: the fidelity baseline needs that one title patch; report it as such.
- If neither F0 nor F1 enters the world: stop, send `stderr.log`, the `render size:` lines and `crashes.log`, do not retry the same
  settings more than once.
- Do not compare a Metal-backend run with a D3DMetal run unless both used the same profile; use `--print-env` diff to show the setup is
  the same but for the backend.

## Report back (one page)

Per run: name, backend, profile, flags, commit, exit/end state, world reached (time), frames in world; table of internal size, output
size, frame time median/p95, GPU memory max, swap max, `gpu_recovery_count` delta; the log lines that settle each of the five
metrics; open items (anything the plan could not decide).

## Known limits

- The render-size rule is unit-tested only; run C settles it. The `settings.xml` key mapping and the pool-check expectation are
  from code reading (`PATCHES.md`, "Which `settings.xml` keys decide the output").
- `--sample` counts RSS of every `victory.exe` on the machine; with `--allow-concurrent` the numbers mix games.
- GPU timestamps read 0 under D3DMetal: the host's GPU millisecond figure is submission/fence time, not shader time. Compare
  profiles by the same figure, not as absolute GPU load.
