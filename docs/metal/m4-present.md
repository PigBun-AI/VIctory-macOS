# M4 present: swap chain, window surface, the game on our backend

Status 2026-11-27. Commits on branch `m4-present` (not pushed): proto v3 swap chain calls, back
swap chain, bridge surface provider, front `IDXGISwapChain4`, Wine test, `run.sh`.

## Design

```
application            front (dxgi.dll)            wire (proto v3)          back (Metal 4)
CreateSwapChainForHwnd ─ SwapChain::create ──────► CREATE_SWAPCHAIN(window id, size, format, n)
                                                    │ back asks its SurfaceProvider for the layer of the window id
GetBuffer(i)           ─ cached ResourceObj         │ back buffers = our own textures (normal ResIds, DESTROY_TEXTURE by the front)
Present(n, flags)      ─ SwapChain::present ─────► PRESENT(queue, swapchain, buffer, interval, flags)
                                                    │ latency wait → waitForDrawable → copy kernel (back buffer → drawable,
                                                    │ nearest scaling, BGRA8 / RGBA16F) → signalDrawable → present
ResizeBuffers          ─ destroy old, RESIZE ─────► RESIZE_SWAPCHAIN (drawableSize follows)
SetMaximumFrameLatency ─────────────────────────► SET_SWAPCHAIN_LATENCY
```

* SyncOps 0x60–0x65 (`CREATE/RESIZE/DESTROY_SWAPCHAIN`, `PRESENT`, `SWAPCHAIN_INFO`,
  `SET_SWAPCHAIN_LATENCY`); `PRESENT` is a sync call, not a stream opcode (0x0700 range unused).
  Everything crossing the wire is a replayable value: the window is the `HWND` number, buffers are
  ids handed out by deterministic counters. No pointer crosses: the layer is fetched by the back
  from the `SurfaceProvider` it was constructed with (the bridge's, or a test's), keyed by the
  window id. The bridge builds the executor and hands it the provider once, at construction; every
  later operation on the back is a `Link::call` / `submit`.
* Frames in flight: 3 by default (the layer's `maximumDrawableCount`), 1 with a frame-latency
  waitable object; `SetMaximumFrameLatency` 1..16, the back keeps up to 8. The back blocks `Present`
  on its own timeline when too many frames are queued; the waitable object is always signalled.
* Flip model only (`FLIP_DISCARD`, `FLIP_SEQUENTIAL`), 2..16 buffers, 1 sample, formats
  R8G8B8A8, B8G8R8A8, R10G10B10A2 (shown through a BGRA8 surface), R16G16B16A16F.
  Windowed only: `SetFullscreenState` is accepted and ignored, no outputs are enumerated.
* `CAMetalLayer.residencySet` is attached to the queue once per queue/layer.
* Test aid: `swapchain_flags::PROBE` makes the back read the centre texel of the drawable after the
  present and wait for the GPU; `SWAPCHAIN_INFO` reports it. Front: `VICTORY_METAL_PROBE_PRESENT=1`.

### Bridge: Wine surface provider (`bridge/src/unix.rs`, module `wine`)

winemac.drv exports `macdrv_functions` (a table of 24 pointers to `my_*` wrappers, `d3dmetal.c`).
Slots are no longer trusted: `Table::find` checks that the symbol is in a loaded `winemac.so`, that
each of the 8 used slots points into the same image and, when the image keeps its local symbols
(CrossOver's does), that the symbol there is the expected `my_*` wrapper; `get_win_data` must return a
record starting with the asked `HWND` with a client view at offset 24. Anything else gives a one-line
reason on stderr ("macdrv_functions[4] is `X`, this bridge expects `my_macdrv_create_metal_device`: …")
and the swap chain call fails with `DXGI_ERROR_UNSUPPORTED`. The check also runs at startup
(visible with `VICTORY_METAL_LOG=1`). Sequence per window, as D3DMetal does it: get_win_data →
create device → create view → get layer → release_win_data. `release_layer` gives view and device back.

## Tests

* back, native: `back/tests/exec_present.rs` (6): colour of every frame read back from the drawable,
  BGRA8 and R16F, resize, scaling, refusals, latency.
* front core: `swapchain.rs` unit tests over `MockLink` (5); bridge: table check without winemac.
* Wine: `tools/metal/test-present.sh --with-d3dmetal`: `front-present.exe` makes a window, a
  flip-model swap chain (size from the window), paints one colour per frame (upload → CopyTextureRegion),
  reads the back buffer back, presents, grows the window, `ResizeBuffers(0,0,0,…)`, again.
  Result: ours 10/10 and 11/11 checks, drawable probe 12/12 and 10/10 frames carry the painted colour
  and size (including after the resize); D3DMetal passes the same exe.
* Visible check (`--screenshot`): `screencapture` of the screen while the window held one colour:
  the window "victory-metal present test" (800x450 after the resize) showed solid orange (255,136,0),
  as painted; title bar and frame normal. The screenshot was outside the repo and deleted.
  A second look at the game window (below) showed the window with the right title and size, black.

## The game on our backend (`tools/mac/run.sh --backend metal`)

Since mac/livetr 4dc9fcaf the one launcher is `tools/mac/run.sh` (`docs/mac/RUN.md`); `tools/metal/run.sh` is a thin wrapper
(`tools/mac/run.sh --backend metal`; it sets no game environment; `--timeout`, `--out`, `--keys`, `--input`, `--exec` are
passed on, `--no-build` skips the `--build` the wrapper adds, `--lenient` is gone: set `VICTORY_METAL_LENIENT=1` in the
environment). The launcher puts our dlls into `target/wineprefix-livetr-metal` (made on first use; the D3DMetal prefixes
are refused), sets `WINEDLLOVERRIDES` (ending in `d3d12,dxgi=n,b`), `WINEDLLPATH`, `VICTORY_METAL_LOG=1`,
`VICTORY_METAL_NOTIMPL_LOG`, and writes logs and `summary.txt` into the out directory (the wrapper's default:
`research/game-runs/<time>-metal`, outside the repo). Default time limit 900 s. The text below describes the first runs of the
old script (`--lenient` etc.) as they were.

### What the game reached (3 runs, 31 s / 300 s / 200 s)

| run | result |
|---|---|
| strict (no `--lenient`) | host device and library up, 1641 start-early pipelines begin translating, then the first list with `ClearRenderTargetView` fails to close (E_NOTIMPL) → "VIctory: host GPU failure", exit 5 after 31 s. No window yet. |
| `--lenient` | window "Americas \| VIctory 30 fps 33.3 ms/frame 8 draws/frame" (1280x720) created through our `CreateSwapChain*`; steady 30 fps presents for the whole run (no hang, no crash, no back error); frames are black (no draw encoding yet). No title card / menu / world: nothing is rendered. Killed by the time limit (exit 143). |

Not reached: title screen, menu, world (they need render). No crash in the lenient runs. The strict
failure is the first missing graphics command, not a defect of present.

### Gaps, by number of calls (lenient run, 200 s, 256 383 lines)

| calls | what | owner |
|---|---|---|
| 45 264 | `SetGraphicsRoot32BitConstants` | render |
| 39 516 | `DrawIndexedInstanced` | render |
| 39 515 | `IASetVertexBuffers`, `IASetIndexBuffer` | render |
| 22 793 | `IASetPrimitiveTopology` | render |
| 22 690 | `SetGraphicsRootSignature` | render |
| 11 493 | `OMSetRenderTargets` | render |
| 5 747 each | `RSSetViewports`, `RSSetScissorRects`, `OMSetStencilRef`, `OMSetBlendFactor`, `DrawInstanced`, `ClearRenderTargetView` | render |
| 846 | `ID3D12Device10::CreateCommittedResource3` (castable formats; victory-gpu falls back) | render/textures (small) |
| 244 | pipeline state refused: geometry shader | render |
| 16 | `CreatePlacedResource2` | render/textures |
| 11 | back: `CREATE_GRAPHICS_PIPELINE: conservative rasterization (Metal has none)` | render |
| 1 | back: pixel shader writes 1 render target, pipeline declares 0 | render |
| – | mesh pipelines (`host mesh pipeline refused`, E_NOTIMPL): the game uses mesh shaders | render (mesh) |
| – | 37 compute shaders do not translate in victory-shader (wave64 on 32 lanes: first-lane read, lane mask as data, s_barrier after early exit) | compute / shader translation |
| – | `LoadPipeline`/`LoadGraphicsPipeline` answer E_INVALIDARG (empty library; 813+309+101 calls; intended) | – |

Present: nothing missing. `IDXGIFactory::CreateSwapChain*` and swap chain methods are not in the list.
Back `last_error`: none from present. `EnumOutputs` answers NOT_FOUND (no outputs) and the game did not mind.

## Diagnostics: `VICTORY_METAL_DUMP_PRESENT=<present numbers>@<directory>`

e.g. `300,1322@/path/outside/the/repo`. At those presents (the n-th present of a swap chain, from 1; for the title's single
swap chain its present number) the back writes `present-<n>-source.png` (the source back buffer, read by a compute kernel
into a readback buffer, as RGBA8) and `present-<n>-drawable.png` (the whole drawable, after the present kernel and before
the display gets it), uncompressed PNGs (`metal/back/src/dump.rs`). Equal sizes: byte-equal; otherwise the drawable is the
nearest-neighbour scaling of the source. Test: `metal/back/tests/exec_present_dump.rs`. The layer's gravity is `resizeAspect`
and `VICTORY_METAL_LOG=1` prints the swap chain (front) and the layer (back: bounds, contentsScale, drawableSize, gravity).

## Known limits

`PRESENT` holds the executor state lock while it waits for the latency slot; `sync_interval > 1` is treated as
1; no HDR or colour management; R10G10B10A2 goes through a BGRA8 surface; exclusive fullscreen and frame
statistics are not implemented; the swap chain keeps the window's layer until `DESTROY_SWAPCHAIN`.
