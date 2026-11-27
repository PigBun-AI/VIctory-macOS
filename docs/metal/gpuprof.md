# GPU time by what ran (`VICTORY_METAL_GPUPROF`) and what a failed submission says

Written 2026-11-28 (branch `fid-perf`). Code: `metal/back/src/prof.rs`, `metal/back/src/fault.rs`,
hooks in `record.rs`, `exec/stream.rs`, `queue.rs`. Tests: `back/tests/gpuprof.rs`,
`gpuprof_stress.rs`, `exec_signal_after_failure.rs`.

## Why

The commit feedback of a submission says only how long the whole submission took
(`Queue::gpu_time_ns`). The front's `timestamps.rs` fakes D3D12 timestamps with CPU time. Nothing
said *which* pipeline, copy or pass takes the card's time, which is what a GPU-bound frame (the
fidelity profile: 2560x1440 internal, 3840x2160 output) needs.

## Switches (all off by default; off costs one atomic read per command)

| variable | meaning |
|---|---|
| `VICTORY_METAL_GPUPROF=<N>` | on; print the top-20 table every N presents (1 = every frame) |
| `VICTORY_METAL_GPUPROF_SECS=<s>` | also print every s seconds (a replay presents nothing) |
| `VICTORY_METAL_GPUPROF_MIN_MS=<ms>` | a window whose scopes add up to less than this per frame is not printed (default 50: menus, loading) |
| `VICTORY_METAL_GPUPROF_SLICE=<n>` | samples one list can take, 64..4096, power of two (default 512). 4096 = only 12 lists sampled at once, the recorder waits for the card |
| `VICTORY_METAL_GPUPROF_RELAXED=1` | relaxed timestamps: less disturbance, ops of an encoder then show its time at the first |
| `VICTORY_METAL_GPUPROF_DRAWS=1` | a sample after every draw (slow) |
| `VICTORY_METAL_GPUPROF_RAW=<n>` | print the raw samples of the first n lists that have a scope >= 1 s (`_RAW_ALL=1`: of any list) |
| `VICTORY_METAL_COMPILE_LOG_MS=<ms>` | pipeline creations slower than this are logged (default 100, always on): `victory-metal compile: cs <hash> took .. ms (.. ms of it in the Metal compiler)` |

Output (stderr): `victory-metal gpuprof: presents a..b (n frames), L lists read, ... ms/frame in scopes`, then 20
rows: ms per frame, share, count per frame, max, mean, label. A single scope >= 1 s is printed at
once (`a scope took .. ms on the card: <label> (list of K scopes, queued .. s ago)`).

Labels: `cs <hash>` and `execute-indirect cs <hash>` (hash = FNV-1a of the DXIL stage bytes, the same
hash as the drop statistics' `back-pso.csv`; `VICTORY_SHADER_SAVE=<dir>` + an FNV pass over the
`*.dxil` maps it to the title's container hash), `copy buf->buf N bytes`, `copy-texture [..] <- [..]`,
`copy-resource`, `fill`, `clear-uav`, `pass WxH colors=[res ids] depth=<res id> draws=n` (one scope per
render encoder), `gap before an encoder`.

## How it works

Each command list takes a slice of one of 12 pooled Metal 4 timestamp counter heaps (4096 entries each; the
device refuses more than about a dozen heaps and sizes above 4096: `AGX: MTLCounterHeap: Invalid heap
size requested.`, measured on an M5 Max, macOS 27.2). The executor writes a timestamp
(`writeTimestampWithGranularity:intoHeap:atIndex:`, render: `..afterStage:..`) into the open
encoder after each compute-side op, after the barriers that open an encoder (`gap`: the wait for earlier
work), and at the end of each render encoder. The scope that ends at sample i is `ts[i]-ts[i-1]`. When the
timeline says the list is done, the slice is read (`resolveCounterRange:`), invalidated and
returned. Ticks to seconds: `queryTimestampFrequency` (24 MHz here); checked against wall time (3
dispatches: 427 ms in the table, 448 ms wall).

Things measured that the numbers depend on:

* A run of *consecutive copies* with no dispatch between them gets only its first few samples
  written; the rest read as zero (`N samples never written` in the header). The time up to the next
  written sample is shared equally between the scopes since the last written one. Copies between dispatches are
  all written. Dispatches and passes always are.
* A slice entry nobody wrote still holds what an earlier list left: a sample older than the list's first
  one is treated as unwritten.
* Precise timestamps serialise the work around them: the *shares* are right, the sum is more than an
  unprofiled frame. `gap` includes the time the card waited for the CPU to submit the list.
* Lists beyond the slice size lose their later scopes (`scopes beyond a list's N samples not counted`).

## What a failed submission says (always on / `VICTORY_METAL_LOG`)

* A command buffer that fails (timeout, page fault, out of memory) used to be logged only with
  `VICTORY_METAL_LOG` and without any hint of what ran. Now, always:
  `victory-metal back: submission <timeline value> FAILED: <domain> <code>: <message> (GPU time ..)`, the
  error's `userInfo` (Metal puts the encoder infos there), and, with `VICTORY_METAL_LOG=1`, the journal
  of the lists in the submission (runs of `set/dispatch/execute-indirect cs <hash>`, copies, fills).
* The timeline is raised to the failed submission's value from the CPU (`Timeline::raise_to`, never
  lowers), so whoever waits for it is released even if the queue's own signal after the commit does not
  come after an error. (Whether it comes was not testable: a write to an unmapped address, which was
  tried, does not fail on this GPU.)
* A command that fails in the middle of a block no longer takes the block's fence signals with it
  (`exec/stream.rs`): the lists finished before it are submitted and the `QueueSignal`s later in the block
  are given, logged as `after <opcode> failed, the fence signal of queue .. is still given`. Before,
  the block was dropped and a fence the title waited for was never signalled (0% CPU, no log).
  `exec_signal_after_failure.rs` fails without the change.
