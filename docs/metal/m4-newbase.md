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
