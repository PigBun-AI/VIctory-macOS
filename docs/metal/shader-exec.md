# `victory-shader-exec`: the Mac host of victory-shader's hardware proofs

`metal/shader-exec` (crate `victory-metal-shader-exec`, binary
`victory-shader-exec`). Branch `harness-back` (the Mac side is
`mac/shader-harness`, patch S6).

**What.** The tests of the shader translator run a translated DXIL program on
a device and compare what it wrote with a CPU reference. On Windows that is
D3D12. On a Mac the test host sends the program and its bindings over a pipe
to this process, which converts the DXIL with Metal Shader Converter,
makes a Metal 4 compute pipeline, fills a descriptor heap and the top-level
argument buffer the converter laid out, dispatches, and returns every UAV's
contents. It uses `victory-metal-back`'s plain-Rust API only, so this crate
names no Metal type.

**Why a process.** MSC 4.0.3 can hang or abort on a program it dislikes; one
bad program must cost one test, not the run. The test host kills and respawns
a worker that dies or exceeds `VICTORY_SHADER_EXEC_TIMEOUT_SECS` (default
120). It also keeps `victory-shader` free of `victory-metal-*` (layering).

**Protocol (version 1).** Frames: `u32` length, payload. Request: `u8 op`
(1 hello, 2 compile, 3 dispatch) and the body (root signature, DXIL, and for
dispatch the root arguments, views, samplers, group counts). Reply: `u8
status` (0 ok, 1 unsupported -> the test skips, 2 failed -> the test fails),
then the body or a text. The codec is written twice (`src/wire.rs`,
`src/req.rs` here; `emit/host_exec.rs` in victory-shader); one golden vector
pins the root signature bytes on each side. Compiles are cached by the hash
of the request prefix (root signature + DXIL).

**Compile flags.** `BOUNDS_CHECK | DISABLE_NAN_INF_OPTIMIZATION`; the second
because the proofs compare NaN and infinity results with D3D12's (without it
`cvt` of a half NaN to int is the int-min, with it 0 as on D3D12). Override
with a decimal `VICTORY_SHADER_EXEC_COMPAT`.

**Run.** `tools/mac/shader-hw.sh --back <this checkout>/metal` in the Mac
worktree. Without `VICTORY_SHADER_EXEC` the tests skip.

**Not covered yet.** Graphics (draw, mesh) tests, wave-64 hardware proofs
(MSC refuses a non-32 wave size; the translator's wave-32 lowering is what
runs), f64 (MSC cannot convert double precision), WARP.
