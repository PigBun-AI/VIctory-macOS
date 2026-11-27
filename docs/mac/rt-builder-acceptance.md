# Batch acceptance: the ray-tracing structure builder on (`VICTORY_BVH_BUILDER=1`)

Plan for one batch of game runs; nothing here runs by itself. Base: `mac/livetr` 12289486 (the build of `tools/mac/check.sh`).
Written 2026-11-27 from the code: `pipeline.rs` (`Builder`, `of_builder`, `builder_runs`, `builder_quarantined`),
`pipeline/make.rs` (`left_out`), `draw.rs` (dispatch gate, `writes`), `memory.rs`, `mac/drop_stats.rs`.

## Before the batch: what must be true

1. **The 53 builder programs translate.** The builder is *all or none*: it runs only while every pipeline the title created with a
   spilled user-data table (`of_builder`, `XboxBvhBuilder.dll`, 53 of them) has a host pipeline. The three wave64 first-lane programs
   (RVA 0x99160, 0x9a830, 0x9be80) are what blocked it; their fix is the wave64 line (`mac/wave64`, S3). Do not start the batch
   before that fix is in the exe, or the first two checks below fail by construction (builder stays off, `missing=3 of=53`).
2. An exe built from that tree (`tools/mac/check.sh`, release). Drop statistics need no diagnostics build.
3. Warm caches: with the builder on, `left_out` stops skipping `CS_RaytracingResolve` (~1000 pipelines) and
   `CS_ScreenProbes_ActivateRays`, and the first builder dispatch makes all 53 pipelines on the recording thread. A cold cache can run
   into the title's 30 s GPU watchdog (Windows line `ec648b34`, which is not in `mac/livetr`). So: one throw-away run first
   (same command, result ignored; D3DMetal's own pipeline cache lives outside our cache folder), then the batch. If the throw-away run
   dies at 30 s with 0xc0000005, that is finding F1 below, not a reason to loosen the checks.
4. No other `victory.exe` running (the script runs one game at a time; `run.sh` refuses a second).

## The command

```sh
tools/mac/rt-builder-accept.sh print --baseline        # the four run.sh lines, nothing started
tools/mac/rt-builder-accept.sh run --baseline          # d3dmetal and metal, each builder on, then builder off (same input)
tools/mac/rt-builder-accept.sh check target/mac-runs/<dir>/*/stderr.log   # logs only
```

Each run is `VICTORY_MAC_DROP_STATS=1 [VICTORY_BVH_BUILDER=1 VICTORY_BVH_QUARANTINE=1] tools/mac/run.sh --backend <b> --perf --timeout 900
--sample 10 --frametime-log --out <dir>` (default input: `tools/mac/input/world-neutral.txt`, Enter keys; profile perf = 1280x720,
non-fidelity; no fidelity result may cite this batch). Four runs: ~1 h with the 900 s limit. `--backends metal` or `-- <run.sh options>`
narrow or extend it. The `on` runs write `<dir>/accept.txt` (the check output); the exit status is non-zero if any check fails.
The metal backend needs its stage (`--build`, `docs/mac/RUN.md`).

## What the log must show (checks, with the log line each reads)

`stderr.log` of the run. All lines are `INFO` unless said (a release exe logs info and above).

| # | Check | Line / source | Pass |
|---|---|---|---|
| 1 | The builder really ran, with all of its programs | `victory_gpu::pipeline: the ray-tracing structure builder runs: all of its programs have host pipelines` with `pipelines=53` (printed when the group is whole; again with a larger number if the title creates more later) | present, `pipelines=53` |
| 2 | No program is missing | `WARN ... the ray-tracing structure builder stays off: not all of its programs have a host pipeline` with `missing=k of=n which=[hashes]` | absent. If present: `which` lists the failing builder pipelines by shader hash; that list goes back to the translator line |
| 3 | No stage was lost after it started | `WARN ... the ray-tracing structure builder rests for this frame and the next` (a dispatch of a stage could not be bound; the others stop for two frames) | absent (up to a handful in the first frames is a finding to read, not a pass) |
| 4 | `rt_builder_off` is zero | `drop-stats summary span=all ... why[... rt_builder_off=N ...]` (dispatches) and, for ExecuteIndirect, `drop-stats top ... why=ind:rt_builder_off` (class `ind` hides it in `why[]`) | `rt_builder_off` absent or 0 once the builder is whole. The whole-run count includes the frames *before* the group was whole (the title creates builder pipelines during loading and dispatches only later; see "Reading the numbers"); the check also requires no `rt_builder_off`/`left_out` in the per-present lines after the `runs` line |
| 5 | `never_requested:left_out` is zero | same summary, `never_requested=N`; top line `why=never_requested:left_out e.g. ray activation shader without BVH, left out` | 0. (`left_out` is evaluated at pipeline creation from `VICTORY_BVH_BUILDER=1`, so with the variable set it must be 0 from the first frame) |
| 6 | The title was not killed | `== time limit 900s reached` in `stderr.log` (run.sh writes it only when the limit ended the run); no `0xc0000005`, no `kIOGPU`/`SubmissionsIgnored`; `summary.txt` "GPU errors" 0 | present / none. Compare with the builder-off run: a death only in the `on` run is F2 |
| 7 | By-address memory worked | `title memory by address: a shader reads or writes through raw addresses` (once, when the first builder dispatch needs it), then every 10 s `title memory by address: N pages ... per frame ... that build ...` with "that build" > 0 | present, builds > 0 |
| 8 | Lost accesses are not rising | `WARN title memory by address: shaders touched pages that had no copy ... frame= pages= made= held=` | few; not more than in the builder-off run (`check` prints the count of both). Each is a read that gave 0 or a dropped store |
| 9 | Cost | `ms/frame` lines, `frametime.log`, `mem.csv` | report only: builder on vs off per backend (7900XTX: ~+40 ms/frame; Mac: no number yet) |

## Garbage into title memory: guard and detection

*Guard (`VICTORY_BVH_QUARANTINE=1`).* Every dispatch of a builder pipeline sets `writes=false` (`draw.rs`, `builder_quarantined`):
what it stores through root UAVs is not copied back (`gpu_writes` is skipped), and the small title buffers it was given do not come
back either (`Use::Builds{back:false}`, `memory.rs`). The structures themselves live in the by-address arenas on the card
(host copies) and are never written into title memory by the builder, so a stage that stores where it should not (run 98: bounding-box
bit patterns used as pointers) cannot reach the title's neighbouring objects. The shared rule on top: no stage runs unless all 53 exist.

*What it is not.* The title also reads results back (compacted sizes, counters); with quarantine it gets none, and a copy's pages are
overwritten from title memory when the title writes them. So quarantine proves "the builder runs on the whole set without killing the
title" and nothing about image correctness; the traces read the host copies, so lighting may already change but is not a result.
Running without quarantine is a separate, later batch (below).

*Detection.*
- The title survives the whole limit in the `on` runs and the checks 6 and 8 above. A write that escaped would show as a death within
  seconds of the first builder submission (run 98: under a second), `0xc0000005` in the loader's crash report, or corrupt geometry
  that is also in the builder-off run's capture (compare).
- Builder-off run of the same input and backend is the control for crashes, GPU errors, `no copy` pages and frame counts.
- With a diagnostics build only (not the default exe): `VICTORY_BVH_TRACE=1` logs `bvh trace: ... size of ... asked ... -> ...`
  (compacted sizes asked and when title memory has them), `VICTORY_MEMORY_DUMP=<folder>` writes the arenas on demand. Use after a
  failed check, not in the batch.
- Not detectable here: a store inside the card's copies that is wrong but harmless to the title (needs the CPU reference of
  `victory-shader/src/bvh.rs` or a Windows hardware comparison, which is out of scope).

## Reading the numbers

- `rt_builder_off` is counted for every dispatch of a builder pipeline while `builder_runs()` is false: from the first dispatch until the
  group is whole, and again for two frames after a "rests". A small, bounded count at the start is expected; what must hold is that it
  stops growing after the `runs` line (check 4 looks at the presents after it). If it keeps growing: the builder is off (check 2).
- `never_requested:left_out` > 0 means the exe did not see `VICTORY_BVH_BUILDER=1` (the run was not started through the script's
  environment) or `VICTORY_ACTIVATE_RAYS=0` is set.
- Expected side effects of the `on` run, not failures: the ~1000 `CS_RaytracingResolve` pipelines are now made (compile time, and
  `compiling` drops rise on a cold cache); `failed:shader_translation(compute_02384a60fda6f2d0)` (non-builder, wave64) is separate.
  On D3DMetal count `marking PSO(n) as no-op` lines in `stdout.log` for compute: the shared layer counts such a dispatch as run.

## Findings to report back (what each failure means)

| Finding | Meaning | Goes to |
|---|---|---|
| F1 death at ~30 s after the first builder dispatch | pipelines made on the recording thread beyond the title's watchdog | port of `ec648b34` (shared layer; ask for it) |
| F2 death only with the builder on, after first submission | a stage stores outside its structures even under quarantine, or a host-copy fault | by-address owner; run with the diagnostics build and `VICTORY_BVH_TRACE=1` |
| `stays off`, `missing=k` | k builder programs do not translate or do not compile | `which=[...]` to the translator (wave64 line) or the backend owner |
| `rests` | a dispatch of a builder stage could not be bound (root signature / descriptors) | `bind_failed` examples in `drop-stats top` |
| `no copy` pages up sharply | by-address page limits (`VICTORY_MEMORY_CARD_MB`, `MOST`), D3DMetal page cache variant is not used (`VICTORY_RT_PAGECACHE` stays unset) | by-address owner |

## After a pass

1. Same batch with `VICTORY_BVH_QUARANTINE` unset, builder on, one backend first (D3DMetal), a shorter limit, someone watching:
   this is the run in which garbage could reach the title. Only after the `on+quarantine` batch passes all checks on that backend.
2. Capture replay (`--corpus`, non-fidelity) with builder on vs off for a lighting comparison: no builder-related capture corpus exists
   yet (`research/captures` was made with it off), so this needs the first corpus recorded with the builder on. Known: captures made
   with the builder on or off hung the card in replay on the Windows line (`docs/fork-ray-tracing.md`).
3. Only then: matrix rows `shared.rt.bvh_build` etc. to `proven/emulated` for the backends that passed; making the builder a default is a
   separate decision (Windows line keeps it off; `VICTORY_BVH_BUILDER` remains the switch).
