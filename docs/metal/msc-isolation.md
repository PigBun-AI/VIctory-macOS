# Isolating Metal Shader Converter in a helper process (proposal, 2026-11-28)

Status: **plan, nothing implemented.** Written after the f64 guard (`msc/guard.rs`, docs/metal/msc.md); to be
confirmed before work starts.

## Why

MSC 4.0.3 reports some bad inputs with `metal-shaderconverter: [..] Fatal: ...` and `abort()` instead of an
error code. The f64 case (`getMinMaxConstants:8246`) was found because five shaders stalled the game. The guard
recognises double precision and the census denylist recognises what a census already saw, but the next shader
that reaches another `Fatal` (or an endless loop) in a path nobody tried stalls the game again, and under
Wine the abort does not even end the process (qualify run 20261128-0217: two Fatal lines, then 14 minutes at
300 % CPU). In-process checks can only list what is known. A separate process turns every MSC death into an
ordinary refusal.

What MSC can do to a process today (measured or read from its output):

| Behaviour | Seen | Caught in process? |
|---|---|---|
| error code (`IRErrorCode`) | many (wave size, unsupported instruction ...) | yes (already `MscError::Rejected`) |
| `Fatal:` message + `SIGABRT` | double-typed wave reduction, 5 shaders | no |
| endless loop / very long run | not seen; survey max 0.5 s, census max 0.2 s per shader | no |
| memory blow-up | not seen | no |
| Metal assertion while building the pipeline (`MTL4Compiler`, after MSC) | 29 geometry pipelines in an older census (`ProcessAbort`) | no, but it is a different stage: see "Scope" |

## Shape

One resident native helper process per converting worker, speaking a small request/response protocol over a
pipe pair, with the payload in a shared file mapping.

* **Binary** `victory-metal-msc-helper`, a new bin of `metal/back` (or its own crate), built for
  `aarch64-apple-darwin` and put in the stage beside `libvictory_metal_unixlib.so` (`wine-stage`). It links only
  `msc::Msc` (dlopen of `libmetalirconverter.dylib`) and the DXBC converter wrapper; no Metal device.
  Native arm64 is preferred over x86_64: no Rosetta tax, MSC is a universal dylib (checked for the f64 work).
* **Spawn** from the unixlib (the x86_64 Mach-O Wine loads) with `posix_spawn`, using
  `posix_spawnattr_setbinpref_np(CPU_TYPE_ARM64)` so the arm64 slice runs from a Rosetta parent. Path: next to
  the loaded unixlib (`dladdr` of `__wine_unix_call_funcs`), overridable by `VICTORY_METAL_MSC_HELPER`.
  `POSIX_SPAWN_CLOEXEC_DEFAULT` so the helper inherits only the two pipe ends. Spawn lazily at the first
  conversion (about 5-15 ms for the process plus the `dlopen` of MSC, once); respawn lazily after a death.
  Open item to check under Wine: it installs its own signal handlers, so `SIGCHLD`/`waitpid` use needs a short
  test (use `poll` on the response pipe's hang-up plus `waitpid(WNOHANG)` rather than a `SIGCHLD` handler).
* **Pool**: `N` helpers, one request at a time each (so the culprit of a death is unambiguous). `N` = the
  converting parallelism the front already has (today up to 8 threads converting in `msc_gpu` tests; the game
  mostly converts from 1 to 3 threads). A request takes a free helper, else waits or starts another up to `N`.
* **Request** (in `compile`'s place; the whole `compile` body, including the DXBC converter, moves into the
  helper): `CompileOptions` + root signature + DXIL bytes. **Response**: status + `Converted` (metallib,
  stage-in metallib, `Reflection`). All plain data already (no MSC types cross), encoded with a tiny hand-written
  little-endian codec (no serde dependency; `Reflection` is ~15 fields). Bytes larger than a few KB go through a
  per-helper shared mapping (`mmap` of an unlinked temp file, size grown on demand, fd passed once), only offsets
  and lengths go over the pipe.
* **Timeout** per request, default 30 s (`VICTORY_METAL_MSC_TIMEOUT_MS`): longer than the slowest census shader
  by two orders. On expiry `SIGKILL` the helper, reap it, answer `Unsupported("MSC timed out")`.
* **Death** (EOF on the pipe, signal exit): answer `Unsupported("MSC died: <signal>, <last stderr line>")`
  (helper stderr goes to a pipe the parent drains into the message; this is where the `Fatal:` text is
  captured), respawn lazily, and append the input's hash to a **runtime denylist**
  (`VICTORY_METAL_MSC_DENYLIST_LEARNED`, append-only text of the same format) so the next run does not even
  try, and the census can fold it in. The in-process guard stays in front (it is free and also keeps a
  known-bad shader from costing a helper restart).
* **Fallback**: `VICTORY_METAL_MSC_ISOLATE=0` converts in process as today (census, tests, anywhere a fast loop
  matters more than survival). Default on in the game (set by `tools/mac/run.sh`/`run.local.env`, not in the code).

## Cost

Measured now (census, 300 compute shaders, every 9th, this machine under load 12-15): MSC median 12 ms,
p95 98 ms, max 207 ms; DXIL median 41 KB, max 532 KB; metallib median 41 KB, max 469 KB.

* Per conversion added: two pipe round trips (about 20-50 us each) plus copying the payload into and out of the
  mapping (a memcpy of at most 1 MB: under 0.2 ms). No serialisation of the metallib itself. Expected overhead
  **0.1-0.3 ms per conversion, 1-3 % of the median**; to be measured the same way as the 300-shader sample
  (in-process vs helper, same shaders) before it ships.
* One-off: spawn + `dlopen` per helper (about 10-20 ms, estimate), memory about 30-60 MB resident per helper
  (MSC's LLVM). With `N` = 4 that is a few hundred MB, small next to the game's 20 GB.
* Parallelism is kept (several helpers), which a single resident helper would lose.

## Interplay with the pipeline archive and caches

* The Metal pipeline archive (`archive.rs`, `lookupArchives`) is consulted by the MTL4 compiler **after** MSC: the
  function description it matches is made from the metallib. So an archive hit does **not** avoid the MSC call
  today, and the helper is on that path for every pipeline of a run.
* To make "archive hit, no MSC" true, add a **conversion cache** next to the archive, keyed by
  `hash(DXIL bytes, CompileOptions, root signature, MSC version)` and holding exactly the helper's response
  (metallib, stage-in metallib, reflection), as files under `VICTORY_METAL_ARCHIVE`. Lookup comes before the
  helper call; a hit needs neither MSC nor the helper (no spawn at all on a warm run); a miss converts through the
  helper and stores the response. This is also what a warm start wants regardless of isolation, and it uses
  the same codec as the helper protocol, so the work is shared. The census already knows every shader, so the
  cache can be filled offline (`victory-census convert --archive` writes the pipeline archive today; it would
  write the conversion cache too).
* A shader that crashes the helper is never cached (nothing came back), and its hash goes to the learned
  denylist.

## Scope: what the helper does not cover

* Failures in `MTL4Compiler` pipeline creation (the geometry-pipeline `ProcessAbort` rows) happen in the
  process that owns the Metal device. They would need the same treatment one stage later (compile in the helper
  with its own device, return a pipeline-data blob / archive entry): a bigger change, not part of this plan.
  Today those are refused up front where known (`Unsupported`) and found by `victory-census convert --isolate`.
* A hostile or malformed input could also hurt the helper's DXBC converter (`libdxilconv`); it moves with
  `compile` into the helper, so it is covered.

## Work packages (if confirmed)

1. Codec + helper binary + in-process/helper switch behind the existing `Msc::compile` signature (about a day;
   the signature does not change, so callers are untouched). Tests: kill-on-Fatal (the `msc_f64_guard` DXIL with
   the guard bypassed must come back `Unsupported`, helper respawned, next conversion fine), timeout (a helper
   built with a sleep hook), concurrent conversions, byte-identical results in process vs helper over the
   census sample.
2. Wine spawn check (`SIGCHLD`, pipe fds, helper path from the stage) with `tools/metal/test-wine.sh`.
3. Conversion cache + census writer.
4. Learned denylist + census merge.
5. Measure: overhead on the 300-shader sample and on a world run (`--until-world`), numbers into this file.
