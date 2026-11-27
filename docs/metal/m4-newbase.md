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
| `DiscardResource` | accepted, contents kept (undefined afterwards is allowed) | `shell/ext.rs` |
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
