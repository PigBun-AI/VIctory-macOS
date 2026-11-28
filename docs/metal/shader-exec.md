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
(1 hello, 2 compile, 3 dispatch, 4 draw, 5 pipeline) and the body (root signature, DXIL, and for
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

**Depth texture + comparison sampler (view kind 9).** `View::Depth` is a one-level D32_FLOAT texture
(width, height, w*h floats) read through an R32_FLOAT view; with a comparison sampler (filter
bit 0x80, comparison function) it is what `image_sample_c*` / `SampleCmp` need on Metal
(MSC's `sample_compare` takes a depth texture). The sampler side was there already
(`sampler_spec`: the comparison function and the compare bit).

**Graphics (ops 4 and 5).** A pipeline request carries the root signature, a vertex
or mesh shader, an optional geometry shader, the pixel shader, the render targets' DXGI
formats (RGBA32 float, sint or uint, read from the pixel shader's output signature), a depth
target kind (none, D32, D32+stencil) and the vertex inputs (semantic, index, DXGI format,
offset; one vertex buffer). The executor describes it with the protocol's own
`GraphicsPsoDesc` (`victory-metal-proto`, plain Rust) and the back's
`exec::graphics_pipeline_for_host`, the table the game's `CREATE_GRAPHICS_PIPELINE` uses,
so a test runs the pipeline the game would make. Op 5 only makes the pipeline (a pair MSC or
Metal refuses is an error at that point, as from D3D12). Op 4 draws one triangle (or
dispatches mesh groups; the group count rides in `groups[0]`) into an RGBA32F target of the
given size, with the same bindings as a dispatch (`bind()`: heaps and top-level argument
buffer are shared with it), and returns the target's texels, four dwords each, row by row.
Pipelines are cached by the hash of the request prefix. Hello's flag bit 2 (barycentrics) is
set for every device, but MSC 4.0.3 has no `attributeAtVertex`: such a program is refused
and its test skips.

**Not covered yet.** Root descriptor tables in a draw, MSAA and more than one target,
depth read-back, wave-64 hardware proofs
(MSC refuses a non-32 wave size; the translator's wave-32 lowering is what
runs), f64 (MSC cannot convert double precision), WARP.
