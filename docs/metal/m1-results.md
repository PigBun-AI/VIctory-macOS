# Metal backend, milestone M1: results (2026-11-27)

M1 = the wire protocol, the Metal 4 executor, the bridge and the D3D12/DXGI
front end, connected end to end under Wine, with compute shaders converted
by Metal Shader Converter; victory-shader's GPU tests run on it and are
compared test by test with D3DMetal. Branch `metal-backend`, HEAD at the
time of writing `3d96542` (pushed to the fork). Upstream files touched: 0.

Versions (checked 2026-11-27): macOS 27.2.1, Xcode 27.2, Wine 11.0
(CrossOver 26.3.0 sources), D3DMetal 4.0b3, Metal Shader Converter 4.0 beta 3
(`IR_VERSION` 4.0.3, universal x86_64+arm64 dylib in /usr/local/lib),
objc2-metal 0.3.2, windows 0.62.2.

## What is in place

| Layer | Doc | State |
|---|---|---|
| proto (seam B) | `proto.md` | v1: 24 sync calls, command stream, dump format; round-trip, refusal, fuzz, golden tests |
| back (Metal 4) | `m1-back.md`, `m1-exec.md` | device, buffers, `wrap`, layered residency sets, descriptor heaps (`IRDescriptorTableEntry`), TLAB + constant ring, allocator ring, deferred stage barriers, queue/fence/feedback, proto interpreter, typed buffer views |
| shaders | `msc.md` | DXIL -> MSC -> `MTL4Compiler` compute pipeline; reflection gives TLAB layout and threadgroup size |
| bridge | `front-api-surface.md` §10 | Wine unixlib carries proto blocks; back executor installed in the unixlib |
| front | `front-api-surface.md` | `d3d12.dll`/`dxgi.dll`, 53 M1 methods; root-signature bytes equal to D3DMetal's |

## Tests

- `tools/metal/check.sh`: 13 steps, all pass (layers, touches, tests on
  arm64 and Rosetta, clippy on three targets, fmt, stub check, xtask policy).
- victory-shader under Wine (`--test-threads=1`), ours vs D3DMetal:
  both pass 392 (D3DMetal 448), only D3DMetal 56, only ours 0, both fail 1
  (legacy FMA signed zero, known on D3DMetal), ignored 6. The 56: graphics or
  mesh pipelines and 2D textures (M3 scope), plus 2 half-NaN cases whose cause
  is MSC 4.0's default NaN/Inf optimisation
  (`IRCompatibilityFlagDisableNanInfOptimization` fixes them; not switched on
  globally). All are in `metal/conformance/expected-fail.txt`.
- victory-gpu under Wine (survey for the next steps): ours 412 pass,
  107 fail, 19 ignored; 0 pass only on ours; no hangs, no device loss.
  The failures are unimplemented paths: custom-heap buffers 34, textures 46,
  DXBC helper shaders (MSC takes DXIL only) 19-24, pipeline library 6,
  graphics PSO 1, and one abort caused by an unimplemented `OMSetRenderTargets`.
- Regression list: root native tests 459/0/15 (same as base, victory-shader
  excluded as on pristine main), xtask policy clean, gnullvm clippy clean
  both ways, D3DMetal victory-gpu 519/0/19 (the extra ignored test is
  compiled in only with `VICTORY_HELPER_DXC`).

## Performance (M5 Max)

| Item | Ours | D3DMetal |
|---|---|---|
| empty submission round trip, back, arm64 / Rosetta | 16 / 20 us | - |
| `ExecuteCommandLists` with no lists, spin | 4.5 us | 25.4 us |
| empty list + spin wait | 20.0 us | 28.6 us |
| empty list + event wait | 114.8 us | 47.0 us |
| copy 4 KiB, call only | 10.6 us | 2.4 us |
| copy 4 KiB, spin | 149.2 us | 147.9 us |
| 1000 dispatches recorded / recorded+run | 110 / 379 us | - |

Not tuned yet: the call-only and event-wait cases (synchronous commit in the
call; one waiter thread per event).

## Known issues carried forward

- Static-sampler table order checked for one sampler only.
- Typed buffer views need a 16-byte aligned start (Metal reads from the
  aligned-down address otherwise); `R32G32B32` typed views have no Metal format.
- Pipeline compile cost: Metal's compile of game shaders median ~0.1 s,
  p95 1.2 s; asynchronous compile and an archive cache are needed.
- A missing residency entry cannot be observed failing on this machine
  (it shows only under memory pressure); only the back's guards are tested.
