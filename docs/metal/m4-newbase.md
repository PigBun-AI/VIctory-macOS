# Metal backend on the new base (mac/livetr)

Branch `metal-livetr`, snapshot of `metal-backend` 19c3903 on `mac/livetr`.
This page is what the new base's host layer (`victory-gpu`) asks of the D3D12
front that the old base did not, and what this branch answers. Method: static
scan of the method names `victory-gpu` calls, the two Wine suites
(`victory-gpu`, `victory-shader`) on D3DMetal and on our dlls, and a game run
with `VICTORY_METAL_NOTIMPL_LOG`.

## Answered

| Call | Answer | Where |
|---|---|---|
| `WriteBufferImmediate` (List2) | proto `WRITE_IMMEDIATE` 0x0480 (proto v4): value into a buffer named by GPU address, in command order; modes marker in/out add a full barrier | front `timestamps.rs` + `vamap.rs`, back `exec/stream.rs` |
| `CreateQueryHeap`, `EndQuery(TIMESTAMP)`, `ResolveQueryData` | **approximation, known difference**: see below | front `timestamps.rs`, `shell/ext.rs` |
| `GetClockCalibration` | card tick = same clock as the stamps (ns), PC tick = `QueryPerformanceCounter` | `shell/ext.rs` |
| `SetMarker` / `BeginEvent` / `EndEvent` (list) | `DEBUG_LABEL` (`=text`, `[text`, `]`); text from the UTF-16 and ANSI encodings, packed PIX blobs give an empty text; the back drops labels | `shell/ext.rs` |
| `SetMarker` / `BeginEvent` / `EndEvent` (queue) | accepted | `shell/ext.rs` |
| `EnqueueMakeResident` | fence signalled at once (unified memory, everything resident, like `MakeResident`) | `shell/ext.rs` |
| `SetBackgroundProcessingMode` | `S_OK`, no measurements | `shell/ext.rs` |
| `DiscardResource` | whole resource: store action discard in a render pass (m4-render); otherwise accepted, contents kept | `shell/list.rs` |
| `CheckFeatureSupport(OPTIONS12).EnhancedBarriersSupported` | 0 (was 1): no `ID3D12GraphicsCommandList7`, `victory-gpu` falls back to `ResourceBarrier` | `caps.rs` |
| `CheckFeatureSupport(OPTIONS).TiledResourcesTier` | 0 (was 2): no reserved resources, so the title (and `victory-gpu` tiles.rs `wanted()`) takes the non-tiled path | `caps.rs` |

## Not answered, on purpose

* `CreateReservedResource`, `UpdateTileMappings`, `GetResourceTiling`: `E_NOTIMPL`. `victory-gpu` uses tiles only when
  `VICTORY_LEVELS_ON_DEMAND=1` and the tier is at least 2 (tiles.rs); the tier is reported as 0 now, which is the fallback.
  No sparse textures were made.
* DRED: `D3D12GetDebugInterface` answers `DXGI_ERROR_SDK_COMPONENT_MISSING`; `victory-gpu` uses DRED only in
  diagnostics builds and its calls fail softly.

## Timestamps: the approximation and the basis for the exact version

`EndQuery(TIMESTAMP, i)` stores a strictly increasing tick (nanoseconds since
first use plus one second, so never 0) in the heap when the call is recorded;
`ResolveQueryData` writes the stored ticks into the buffer with
`WRITE_IMMEDIATE`. Values therefore rise in the order of recording and
`GetTimestampFrequency` (1 GHz) is consistent with them, but differences are
CPU record-to-record time, **not GPU time**. Callers that only order stamps or
look for monotonicity (the timeline in `victory-gpu`) work; anything that
reads GPU durations does not.

The exact path (not built): Metal 4, SDK 27.2 (`MacOSX.sdk`, checked
2026-11-27): `MTL4Counters.h` (`MTL4CounterHeap`, `MTL4TimestampGranularity`),
`MTL4ComputeCommandEncoder.h:832`
`-writeTimestampWithGranularity:intoHeap:atIndex:`, and
`MTL4CommandBuffer resolveCounterHeap:withRange:intoBuffer:waitFence:updateFence:`
(`MTL4Counters.h:85`). It needs a new proto opcode pair in 0x0480-0x04FF, a heap
object in the back, and a resolve that happens after the producing work.

## After the migration of 19c3903..32c6f1fd (2026-11-27 21:50)

The old base's m4-render, m4-fix and no-attachment passes are on this branch
(proto v5). What changed for the gaps above:

* `CreateDescriptorHeap` refused one heap: a shader-visible CBV/SRV/UAV heap of
  4,194,304 (2^22) descriptors, which `victory-gpu` creates to probe for raw
  twins of buffer views (`descriptor/host.rs`, `probe_view_limit`). The limit is
  now 2^22 in front and back (`MAX_SHADER_VISIBLE_CBV_SRV_UAV`,
  `MAX_HEAP_SLOTS`; 96 MB of slots at stride 24); the host then reports
  `raw_twins=true`. `VICTORY_METAL_LOG=1` names the refused description.
* "PS writes 1 RT, pipeline declares 0": accepted since the old base's
  34182140, writes dropped; no longer logged.
* BC mips smaller than a block: the old base's copy fixes are in; no copy error
  in a run to the point below.
* Errors logged by the front now carry the real call site (`or_werr`); the 800
  remaining `E_INVALIDARG` lines of a run are `ID3D12PipelineLibrary::Load*`
  misses (the library only stores names; the spec says `E_INVALIDARG`).
* `WriteBufferImmediate` and timestamp resolves end the open render pass first.

Game run (`research/game-runs/20261127-metal-livetr-r4`, no `--lenient`, default
keys, 420 s): account screen passed by the scripted Enter (`run.sh` expands
`8-900/3:Enter` into `8:Enter,11:Enter,...`: the base's parser has no period
form), 33.3 ms/frame (30 fps cap) with 8-11 draws/frame until frame 1069, the
card 5-12 % busy, 0.2-13 ms preparing. At frame 1069 the title starts its world
load (4400 card memory waits, large buffer copies), compiles pipelines, then
stops at Rosetta's `unexpectedly need to EmulateForward on a synchronous
exception x86_rip=0x140456fe2` in the title: the same address and the same
last log line as D3DMetal on this base (A/B in
`20261127-d3dmetal-livetr-ab`, own prefix), so not our backend. First
failures of ours, in order: 105 pipelines refused (103 `MSC refused the shader:
dx.op.attributeAtVertex.f32` = SM 6.1 `GetAttributeAtVertex`, which the
translator emits for flat vertex values, 102 of them mesh pipelines; 2 mesh
pipelines over Metal's 32 KB mesh size, 34816), then the black screen: the
frames that follow draw nothing for those pipelines.

Remaining gaps by count: `dx.op.attributeAtVertex` 103; `OMSetDepthBounds`
(accepted, no effect) 4377; mesh size 34816 > 32768: 2; pipeline library
misses ~800 (expected).

## After m5-gs, attributeAtVertex, replay (2026-11-27 22:35, on mac/livetr d4e89736)

**m5-gs is carried** (indirect GS draws, cross-family BC castable lists refused in the front, CopyResource self
copy; `m5-gs.md`). `run.sh` sets `VICTORY_METAL_SKIP_GS_INDIRECT=1` by default (run.sh only; the code draws them).

**attributeAtVertex, node by node.**

| node | finding | test |
|---|---|---|
| front caps | `CheckFeatureSupport(OPTIONS3)` is D3DMetal's answer byte for byte (`00000000 01000000 0d000000 02000000 01000000`: `BarycentricsSupported` 1, `ViewInstancingTier` 2, `WriteBufferImmediateSupportFlags` 0xd, casting 1); D3DMetal's golden is `front/tests/golden/d3dmetal-wine11.txt` line 146. So the host reports the same capability, and the 103 refusals are not a difference in what is reported | `caps::tests::options3_is_what_d3dmetal_reports_and_says_barycentrics_are_supported` |
| victory-shader (shared) | the op is emitted by `emit/lower.rs` `vertex_input` (`attributeAtVertex.f32/.i32`) whenever the GCN program reads a pixel attribute per vertex (`per_vertex(row)`: computed barycentrics `Interp::Weighted`, `v_interp_p10/p20`, `v_interp_mov` of a non-provoking vertex; decided in `io.rs`). It **never looks at OPTIONS3**; `BarycentricsSupported` is read only by a test (`emit/mod.rs` partial interpolation). No capability switch exists, so no front value can change the path. Shared layer: not touched | - |
| MSC 4.0.3 | refuses `dx.op.attributeAtVertex.f32` (`UnsupportedInstruction`); D3DMetal's own converter accepts it | `msc_features.rs` case `get_attribute_at_vertex` (the instruction), `gfx_pipeline.rs::a_pixel_shader_that_reads_a_vertex_value_is_refused_by_msc_with_its_reason` (the pipeline: `CREATE_GRAPHICS_PIPELINE` says `attributeAtVertex`) |

Game run r6 (below): 330 `attributeAtVertex` refusals in 6.5 minutes. Needs either a translator switch (flat values
through a GS or `nointerpolation` instead of per-vertex reads: shared layer, Windows line) or our own rewrite of the
DXIL before MSC. Not done here.

**Compute pipelines through a state stream.** `ID3D12Device2::CreatePipelineState` with a CS subobject was refused
(`E_INVALIDARG`, "a compute shader in a graphics pipeline state stream"); `victory-gpu` makes some of its own passes
that way (the replay's card compare, DXIL 27de16bed8ae4b32). The list that dispatched it lost its pipeline and
failed (`DISPATCH ... INVALID_ARG`, 5x5x33), so the replay's whole 5-list submission was dropped. Now the front makes a
compute pipeline of it (`pso.rs` carries `cs`; `shell/device.rs` `make_compute_pso`). Node: front; test:
`tests/pso.rs` (stream with a CS parses to `cs`).

**Indirect GS and the black picture.** On this base no frame with an indirect GS draw can be reached (the world load
starts at present 1069; `VICTORY_RECORD_AT` needs such a frame and D3DMetal stops earlier), so no capture of one exists.
What was established:
* the draws' records in the old base's black runs (`{3,0,0,0}`, `{84,0|1,0,0}`, `{4,2034,0,0}`): the back draws the
  same pixels as the direct draws for no, zero, one and 2034 instances
  (`gfx_draw.rs::indirect_gs_records_with_no_many_or_zero_instances_match_the_direct_draws`), so the real-count path
  is right for those shapes: hypothesis (b) of `m5-gs.md` has no support;
* hypothesis (a), data fed by compute shaders that do not translate: the old base's run had about 20 `shader does not
  translate ... draws are dropped` compute shaders (wave64 on 32 lanes, s_barrier after early exit, ... shared
  translator); this run has the same kind (`no wave32 lowering for this wave64 program`). Node: victory-shader.
  Not proven without a frame; a capture of a frame with such a draw is the next step.
* with the indirect GS draws **skipped** (r6) the picture still goes black between intro montage scenes (t=205 s,
  250 s, 340 s) and shows art at others (140 s, 295 s): black alone is not the GS. Whether those are the title's fades
  is not known.
Current state: the code draws indirect GS; `run.sh` skips them.

**Frame workbench step 1** (`tools/metal/replay.sh`): the sample f300 (D3DMetal capture) replayed on our back: PSNR
78.2 dB against the presented picture, 2730 of 921600 pixels differ, by at most 1 level in one channel; 2.90 ms on
the card (3 loops). Before the CS-stream fix: 14.9 dB (5-list submission dropped). Left: 389 resources not made
(first: placed 2D texture 128x1, `A8_UNORM` (65), `CreatePlacedResource` `E_INVALIDARG`), contents left out; no
effect on this picture.

**Game run r6** (`research/game-runs/20261127-metal-livetr-r6`, run.sh defaults, 420 s limit, own prefix): account
screen passed, world frame 1 at present 1322 (`mac::world`), title frame 2155 reached, 2-4 fps (240-640 ms/frame,
300-560 draws/frame, the card ~13 % busy: the submitting thread spends the time, 1.1 GB per frame of shader-write
checks on the CPU, shared layer). **The Rosetta `EmulateForward` crash at 0x140456fe2 did not occur** (past
frame 1069 by 1000 frames), after the safecopy fix in d4e89736. The run ended at 409 s with a title-side `int3`
(`victory-image+0x5149aec`, WER, 0x80004001) while pipelines were failing: 330 attributeAtVertex refusals, 14 mesh
pipelines over 32 KB (35840 and 34816), 52 typed buffer views of format 6 (`R32G32B32_FLOAT`: Metal has no
texture-buffer format; D3DMetal answers with R32G32, its log says so) and 49 failed SRVs (`E_NOTIMPL`).
