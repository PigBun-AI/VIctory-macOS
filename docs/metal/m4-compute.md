# M4 compute: results and design

Branch `m4-compute` (off `metal-backend` 1673774). Scope: the silent errors of
`victory-gpu` on our backend, `ExecuteIndirect` for compute, B8G8R8X8,
`CreateCommittedResource3` / `CreatePlacedResource2`, an in-process
`PipelineLibrary`, and the latency of `ExecuteCommandLists`.

Design rule kept throughout: the front reaches the back only through the
link (`call` / `submit`, proto bytes); every new object is made by a sync
op or a stream command; the back's handles and the order of its work are a
function of the call sequence (no thread scheduling in them), so that a
recording can be replayed and a back swapped.

## ExecuteIndirect

Proto v3 (`PROTO_VERSION` = 3, fingerprint pinned in `proto/tests/version.rs`;
old arms kept). `EXECUTE_INDIRECT` (0x0800) is a stream command that carries
the **command signature itself** (the argument list and the stride; the
front validates it once in `CreateCommandSignature`) plus the argument
buffer, the count buffer, their offsets and `MaxCommandCount`. No signature
object exists on the back, so replaying a stream needs nothing else.
Argument kinds are numbered as the API numbers them (all of them are in
`IndirectArgKind`): the draw, draw-indexed, dispatch-mesh, vertex-buffer and
index-buffer kinds are **reserved** and answer `UNSUPPORTED` in `validate`
until the render encoder lands; the compute kinds (dispatch, constants,
root CBV/SRV/UAV) work.

How it runs (`back/src/indirect.rs`, `back/src/exec/indirect.rs`): a compute
kernel expands the argument buffer on the GPU into what the dispatches need,
in chunks of 256 records (at most 65 536 records per call), and the encoder
issues `dispatchThreadgroupsWithIndirectBuffer` per record, with the root
arguments (constants, root descriptors) copied into the constant ring
segment of that record. The count buffer is read on the GPU and clamps
`MaxCommandCount`; without one, `MaxCommandCount` is the count; with one, it
is clamped to the records that fit the argument buffer. Both buffers are in
the residency set (the buffer table guarantees it).

Why a kernel and not an `MTLIndirectCommandBuffer`: the pipelines made by
Metal Shader Converter are not built with indirect-command support, and read
one top-level argument buffer at a fixed table slot that an ICB command would
have to rebind per command; making every compute pipeline ICB-capable costs
every pipeline, to serve one call. The kernel keeps the pipelines as they
are and works with root-constant-changing signatures, which is the case the
games use. Cost: one extra dispatch per 256 records.

Tests: `back/tests/exec_indirect.rs` (6, arm64 and Rosetta), proto round
trip and rejection tests, `front/tests/indirect.rs`; under Wine
`victory-gpu indirect::` 16 of 16.

## The five silent errors

Wrong results with no error code. Causes and fixes (the diagnostics that
found them: `VICTORY_METAL_LOG=1` names the command that stopped a block,
`VICTORY_METAL_TRACE=1` dumps every decoded block, 710ef84):

* `shadow::tests::widened_views_*` (3): one cause. The list's root signature
  and the pipeline's were made as two objects of the same bytes (D3D12 treats
  them as compatible); the back compared the objects and refused the dispatch
  with `INVALID_ARG`, which `ExecuteCommandLists` has no way to return, so
  the patch passes never ran. Now compatible = same description
  (`compatible_with`). ab13d64; back test in `exec_compat.rs`.
* `indirect::tests::hoisted_batch_*`: the front's state tracker sent one
  barrier for `UNORDERED_ACCESS -> INDIRECT_ARGUMENT -> COPY_SOURCE` (to the
  indirect stages) and none for the second step, so the copy ran without
  waiting for the shader's writes. The tracker keeps the stages that wrote a
  subresource and the stages already made visible, and sends the missing
  ones on any later transition. b046298; `front/src/state.rs` tests.
* `draw::a_depth_texture_written_through_unordered_views_reads_as_depth`
  (stencil reads 0): D3D12's `X32_TYPELESS_G8X24_UINT` view has the stencil in
  G; Metal's stencil plane view has it in R. A plane-1 SRV now sits under the
  swizzle `[Zero, R, Zero, One]` and the application's mapping selects from
  it. 41da51c; back test in `texture_gpu.rs`. (The victory-gpu test then
  stops at a graphics pipeline with blend factor 0: the render hand's.)

## ExecuteCommandLists latency

The commit of a submission and a fence's signal and wait are made by a thread
of the queue (`back/src/queue.rs`, `Submitter`), in the order the calls were
made. The calling thread settles first everything that gives a call its
identity: the timeline value, the constant ring's seal and the allocator's
busy mark. What happens on the GPU therefore does not depend on when the
thread runs, and the values a caller sees are the same as before; a caller
that needs the work done waits on the timeline or the fence (as a D3D12
caller does). The thread spins for 150 µs after a job before it sleeps, so
calls that follow each other closely pay no wake-up, and runs at the top
quality of service.

In the front, `SetEventOnCompletion` no longer starts a thread per wait:
`front/src/fence_wait.rs` keeps one thread per fence that takes the waits in
value order (a wait for a lower value than the one the thread blocks on gets
a thread of its own; values normally rise). The thread leaves after a second
without a wait.

Measured with `tools/metal/test-e2e.sh --runs 5` (microseconds, median of the
per-process p50), D3DMetal and ours side by side in one run:

| case | before: ours | before: D3DMetal | after: ours | after: D3DMetal |
|---|---|---|---|---|
| copy 4 KiB, call only | 11.2 | 2.4 | 10.9 | 2.3 |
| copy 4 KiB, spin | 150.5 | 148.6 | 137.7 | 138.9 |
| empty list + event wait | 114.4 | 72.2 | 37.6 | 48.9 |
| empty list + spin | 20.9 | 28.3 | 21.2 | 29.2 |
| no lists, spin | 6.1 | 37.0 | 9.0 | 27.9 |

(The D3DMetal column moves from run to run with the machine's state; compare
within a run.) The event wait is the gain: 3x, now below D3DMetal. The
call-only case did **not** move: of the 7 µs the back spends in one
`ExecuteCommandLists` of a 4 KiB copy, about 5 µs is Metal recording on the
calling thread (opening the compute encoder alone is 40% of the samples;
`sample` output in the commit discussion), and the commit that moved off the
thread was 2.5 µs of it. Getting near D3DMetal's 2.3 µs needs the recording
itself off the calling thread: the block would be validated and its handles
settled in the call, and the encoding run later in order. That is a change
of when an encoding error is reported (the call has no result in the API, so
it would be a sticky device status), and is not done here.

Order semantics: `back/tests/exec_streams.rs`
`commits_made_off_the_calling_thread_keep_the_order_of_the_calls` (a
200-step copy chain over two queues ordered only by fence signals and waits,
made in one burst without waiting) and all the existing order tests; the
e2e `fence_semantics` and `queue_wait_orders_work` pass under Wine.

## PipelineLibrary

`front/src/library.rs`, `shell/library.rs`. The library holds names and
fingerprints (FNV-1a over the root signature blob, the shader bytes and the
proto description with addresses taken out), no back object. `Store` records
`name -> fingerprint`; `Load*` matches name and description in the front and,
on a match, makes the pipeline through the ordinary create path, where the
back's compile cache makes it cheap; no match is `E_INVALIDARG` ("not in the
library"). `Serialize` writes the entries (v2 blob; the v1 empty blob is
still read), so a later run's library knows the names; the compiled code is
not in the blob (no Metal archive). The two `victory-gpu` library tests that
failed pass.

## B8G8R8X8, CreateCommittedResource3 / CreatePlacedResource2

See `docs/metal/m2-textures.md`, last section.

## Known issues

* `victory-gpu` tests that stop at graphics or render features (not
  compute): `hdr_probe::d3dmetal_probes_graphics_rtv_and_compute_uav_hdr_passes`,
  `pipeline::depth_bounds_test::depth_bounds_control_a_title_draw`,
  `draw::tests::a_depth_texture_written_through_unordered_views_reads_as_depth`
  (its compute half passes; it stops at a graphics pipeline with blend factor
  0) and the aborting `set_targets_caches_keys_and_clears_on_host_state_lost`
  (skipped in the runs). They belong to the render encoder.
* `draw::tests::unordered_1d_and_3d_textures_keep_their_writes`: the 3D
  round passes; the 1D round (`RWTexture1D<float>`, width 4096) reads 0 where
  the shader wrote. It failed the same way before this branch. A back-level
  1D UAV write read through another view works
  (`texture_gpu::a_1d_texture_is_written_and_read_through_views`), the
  descriptors and dispatches are as for 2D, and the back reports nothing;
  what is left is how the converted shader addresses a 1D texture. Open.
* Graphics-side indirect (draw, draw-indexed, dispatch-mesh, vertex and index
  buffer arguments): numbers reserved, `UNSUPPORTED`.
* Castable-format lists are a flag: a view outside the list is not refused.
* The call-only latency (above).
