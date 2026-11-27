# M1 back: measurements

Numbers and how they were taken. Raw output: `~/Documents/GTA6Project/research/probe-results/m1-back-*.txt`
(outside the repository). Machine: Apple M5 Max, macOS 27, Rust 1.98.1 `--release`.
"Rosetta" = the `x86_64-apple-darwin` build run under Rosetta (the unixlib's future architecture).

Reproduce: `cd metal && cargo test --release -p victory-metal-back --test perf -- --ignored --nocapture --test-threads=1`
(natively; add `--target x86_64-apple-darwin` for Rosetta). Source: `metal/back/tests/perf.rs`.

## Timings

| What | Method | arm64 | Rosetta |
|---|---|---|---|
| Empty submit round trip, no inter-submit barrier | `begin_list`, `end`, `submit`, CPU waits on the queue timeline; 2000 runs after 200 warm-up | p50 17 µs, p99 35 µs | p50 21 µs, p99 58 µs |
| Same, with the inter-submit barrier (default), **before** (barrier = an all-stage producer barrier at the end of every list, which made an empty list open an encoder) | the list is one encoder holding one all-stage barrier | p50 109-130 µs, p99 222 µs | p50 111-125 µs, p99 240 µs |
| Same, with the inter-submit barrier (default), **after** (consumer-form queue barrier at the start of each encoder; an empty list opens none) | `begin_list`, `end`, `submit`, wait | p50 16 µs, p99 25 µs | p50 20 µs, p99 33 µs |
| One tiny dispatch per list, round trip, barrier off / on, **before** | `begin_list`, set address, dispatch, `end`, `submit`, wait; 2000 runs after 200 warm-up | off p50 113 / on 119 µs | off 125 / on 123 µs |
| Same, **after** | same | off p50 115 / on 113 µs | off 119 / on 117 µs |
| 400 lists, 1 tiny dispatch each, committed one by one, wall time to the last | best of 5 | off 6.75 ms / on 6.94 ms (+3 %) | off 6.60 / on 6.17 (-6 %) |
| 400 lists, 1 dispatch of 1M threads each | best of 5 | off 7.08 / on 6.52 (-8 %) | off 6.67 / on 7.12 (+7 %) |
| 64 lists, 1 small kernel each (1 threadgroup, ~3.7 ms), independent outputs | best of 5 | off 238.5 ms / on 250.4 ms (x1.0) | off 238.9 / on 250.3 (x1.0) |
| 1000 dispatches in one list, no barriers: record | p50 of 30 | 110 µs (0.11 µs per dispatch) | 103 µs (0.10 µs) |
| same: record + submit + GPU, CPU waits | p50 of 30 | 379 µs | 369 µs |
| 1000 dispatches, a dispatch-to-dispatch barrier between each: record | p50 of 30 | 167 µs (0.17 µs) | 216 µs (0.22 µs) |
| same: record + submit + GPU | p50 of 30 | 1115 µs | 1184 µs |
| Constant ring: allocate + write a 64-byte root-argument block | 4 M allocations, segment sealed as a submission would | 10.5 M/s (95 ns, 0.7 GB/s) | 5.5 M/s (182 ns, 0.4 GB/s) |
| 1000 buffers: create / drop / `maintain` (commit + remove all) | one run | 2 µs, 0.1 µs, 0.60 ms | 3 µs, 0.1 µs, 1.27 ms |

Reading the table:

* **The inter-submit barrier (changed 2026-11-27).** The first version put an
  all-stage barrier at the end of every list (producer form), so an empty
  list needed an encoder to hold it (17 → ~110-130 µs). Now every encoder
  *starts* with `barrierAfterQueueStages:beforeStages:` (consumer form, all
  earlier queue stages → this encoder's stages), nothing is encoded at the
  end, and a list with no commands opens no encoder: the empty round trip is
  back to 16 µs (arm64) / 20 µs (Rosetta), the same as with no barrier. The
  barrier is in every encoder of a list, not only the first: Metal 4 may
  overlap two encoders of one list, so the first's barrier would not order
  the second after earlier lists (it also serialises the encoders of one
  list, which in M1 means nothing: a list has one encoder). A list that
  already has an encoder never paid for an extra one: the one-dispatch rows
  show ~115 µs with the barrier off, before and after: an encoder with real
  work costs ~100 µs of round trip by itself; the 130 µs of the empty list was
  that cost, paid for no work. So the saving is for lists/submits that do
  nothing on the GPU (an `ExecuteCommandLists` with an empty list, a list
  whose commands were all state), not for lists with work. Correctness:
  `tests/gpu.rs`
  `consecutive_lists_are_ordered_by_the_inter_submit_barrier` (one commit)
  and `consecutive_submits_are_ordered_by_the_inter_submit_barrier` (two
  commits and an empty list in between, 10 rounds) assert that the dependent
  kernel reads the new value with the barrier; the observational tests below
  still show 10/10 stale reads without it.

* The conservative inter-submit barrier costs nothing visible in throughput
  (rows 3-5: within run-to-run noise, either sign). It costs one encoder per
  list: an empty list goes from 17 to 130 µs round trip. Only lists that carry
  a real dispatch hide this.
* Rows 3-5 are CPU-bound (about 17 µs per commit), so they cannot show a GPU
  overlap gain; row 5 does have GPU time to overlap, and showed none with the
  barrier off either: 64 independent kernels ran back to back (238 ms is
  64 x 3.7 ms). Whether the barrier can ever be tightened for a gain is
  therefore not shown by this data. The ordering itself is required (below).
* The 1000-dispatch barrier row: 0.7 ms more GPU time for 1000 serialised
  dispatches (0.7 µs each).

## Observations that gate design (not timings)

Run by `tests/gpu.rs`, 10 runs each, identical on both architectures. A slow
kernel A (single thread, 40 M LCG steps) writes a value; kernel B reads it.

| Setup | Result |
|---|---|
| A, B in one encoder, no barrier | 10/10 B read the stale value |
| A, B in two encoders of one list, no barrier | 10/10 stale |
| A in list 1, B in list 2, one `commit`, no barrier | 10/10 stale |
| A, B in two commits, no barrier | 10/10 stale |
| Same setups with the barrier the back inserts (consumer-form head barrier) | always correct (5 runs each in `barrier_between_dependent_dispatches_orders_them`; `consecutive_lists_are_ordered_by_the_inter_submit_barrier`; 10 runs of two commits in `consecutive_submits_are_ordered_by_the_inter_submit_barrier`) |
| Kernel reads a buffer that is in no residency set (shared storage, 4 KiB, idle machine) | read the right value, status healthy: a missing residency entry is **not** observable here, so the tests check by construction (a descriptor or copy naming such a buffer is refused) |
| Buffers destroyed while the kernel using them runs | stay in their residency set (2 retired entries pending), nothing removed until the queue timeline passes the submission; result correct; removed by the next `maintain` |
