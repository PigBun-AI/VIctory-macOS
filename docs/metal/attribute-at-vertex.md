# GetAttributeAtVertex (`dx.op.attributeAtVertex`) on the Metal chain

Research of the `attr` branch, 2026-11-27 (Asia/Shanghai), Apple M5 Max (Apple10), macOS 27.2.1, SDK 27.2,
MSC 4.0.3 (`/usr/local/lib/libmetalirconverter.dylib`), D3DMetal 4.0b3 build 2. Rule: fidelity first, cost recorded only.
Nothing here touches the shared layer; the changes it needs are listed in "Shared-layer changes".
Game-derived data and the research tools stay outside the repository (`research/attr-vertex/`).

## Result in one paragraph

All 4852 pixel programs that MSC refuses read **all three** vertices (`VertexID` 0, 1, 2 in every one; none reads only
the provoking vertex) and weight them with `SV_Barycentrics`. MSC 4.0.3 has no switch for it and neither has D3DMetal's
converter. The route that is exact on every Apple GPU family is to give the pixel stage the three vertex values as flat
inputs: vertex 0 is the existing flat input (the provoking vertex), vertices 1 and 2 are two new packed per-primitive
inputs that the mesh stage fills from a groupshared exchange. Proved here: the pixel half converts 4852 of 4852 programs
through MSC and 4721 through a full Metal pipeline build (the 131 others stop at the census harness's own DXC vertex
shader, not at Metal), and a mesh+pixel pair using the lowering draws the CPU-expected pixels on the GPU. Not yet done:
the translator itself (shared layer) and the vertex-pipeline case.

## The five items

### 1. MSC 4.0.3 switches (read 2026-11-27)
No flag, option or compatibility flag concerns per-vertex values: the compatibility flags are BoundsCheck,
VertexPositionInfToNan, TextureMinLODClamp, SamplerLODBias, PositionInvariance, SampleNanToZero, TexWriteRoundingRTZ,
Suppress2DComputeDerivativeErrors, ForceTextureArray, DisableNanInfOptimization, EnsureTGMemoryCoherency; the others
are geometry/tessellation emulation, input topology, minimum GPU family (Apple6..10, Metal3), stage-in generation and
ray tracing helpers. UserManual.md line 496: "SV_Baricentrics: Yes, Linear interpolation only, GetAttributeAtVertex not
supported". `--help-hidden` adds nothing. The dylib contains the AIR names `air.get_vertex_value[.s/.u]` and
`vertex_value_type`, but the DXIL translation never emits them (strings of the dylib; the converter returns error code 7
= unsupported instruction for `dx.op.attributeAtVertex` with the minimum family at Apple10 / deployment 27.0 as well).
Not explored: hidden C++ exports (`IRCompilerForce4WideVaryings`, `DXIL2AIR::tryLoadReplacementIR`), no documented use.

### 2. Native Metal (MSL spec 4.1 of 2026-06-04, Metal Feature Set Tables of 2026-10-23, headers of SDK 27.2)
* MSL 4.0+ (`__HAVE_VERTEX_VALUE__`) has `vertex_value<T>` (`<metal_vertex_value>`) as a fragment `[[stage_in]]` member;
  `get(vertex_index::first|second|third)`. Feature set tables: "access to pre-raster per-vertex values" is
  **Apple10 only** (M5/A19 family; M1..M4 are not covered). The SDK API headers have no provoking-vertex API.
* Measured on this machine (`research/attr-vertex/tools/vv.swift`, `vvm.swift`; runtime MSL compile with
  languageVersion 4.0; `xcrun metal` is not usable, the Metal Toolchain is not installed): `supportsFamily(.apple10)` true.
  Triangle list ABC: get(first/second/third) = A/B/C and `[[flat]]` = A. Strip ABCD: triangle 1 is (A,B,C), triangle 2
  (B,D,C) with first = flat = B. Mesh pipelines work and `first` = the first *index* of the primitive = the flat value
  (indices (1,2,0): first = vertex 1). So `first` is the provoking vertex and `second/third` follow the rasterised order.
* Native use from the DXIL path needs AIR to carry `air.get_vertex_value`. A PS compiled by MSC could be patched
  (replace three flat args by one `air.vertex_value` pointer argument and `get_vertex_value` calls), and Apple's own
  AIR loads; but the only bitcode writers available here (Homebrew LLVM 23, Xcode clang 21) produce modules the AGX
  compiler rejects (`XPC_ERROR_CONNECTION_INTERRUPTED`, also for an unmodified Apple-made AIR re-written by LLVM 23),
  so the patched PS could not be drawn. Without the Metal Toolchain (or our own bitstream writer) this route is blocked.

### 3. D3DMetal 4.0b3 (read-only analysis of the local copy, not distributed)
Contrary to the brief, D3DMetal does **not** take it: its `libmetalirconverter.dylib` (x86_64) returns error code 7 on
the same DXIL (test: `research/attr-vertex/tools/irtest2.c`, a `GetAttributeAtVertex(c,1)` pixel shader; the same
converter accepts the same shader without the call). Public report: GitHub issue pragyaangaur/VIctory-macOS#203
(read 2026-11-27, M5 Max, macOS 27.3, D3DMetal 4.0b3): the PSO call returns S_OK, D3DMetal logs "fragment compiler error 7"
and "marking PSO(0) as no-op"; a mesh variant works when the three values are passed as flat per-primitive inputs. Not
checked: whether D3DMetal's main binary pre-lowers any shader before it reaches the converter.
D3DMetal's `libdxcompiler` has `TranslateGetAttributeAtVertex` and the DXC rule "attribute must have nointerpolation".

### 4. Rewrite before MSC (the pixel half, measured)
`research/attr-vertex/tools/`: disassemble (D3DMetal's x86_64 `libdxcompiler`, `IDxcCompiler::Disassemble`), rewrite
the text, `IDxcAssembler::AssembleToContainer` (it regenerates ISG1/PSV from the metadata) and give it to MSC.
Rewrite: `attributeAtVertex(sig,row,col,0)` -> `loadInput` of the same element; vertices 1 and 2 -> `loadInput` of new
packed `float4` flat elements (TEXCOORD index 64+n, four components each). MSC then emits `air.flat` inputs for them.
Census data (4853 refused pixel programs; 4852 are attributeAtVertex, 1 is `atomicBinOp.i64`):
`vertex ids` {0,1,2} in 4852/4852; distinct components read from vertices 1/2: median 10, p90 22, p99 29, max 38
(<=8: 2327, <=16: 3625, <=32: 4846). Float only (f32) in all 4852.
This rewrite works as a measurement; it is not shippable in the product (needs DXC, which does not exist on the user's
Mac), so the product form is the translator emitting the same DXIL (route below).

### 5. Translator-side routes
* New base (this branch): 87e9d414 emits `attributeAtVertex` unconditionally (`emit/lower.rs` `vertex_input`,
  `per_vertex`, `eval_channel`; `emit/shader.rs:559`; test `emit/mod.rs::computed_barycentrics_read_the_vertices`).
  No capability switch exists.
* Old base: 4ad2628d (#300, `emit/deltas.rs`, geometry shader exposing P10/P20 as flat inputs) works for vertex
  pipelines only and its host hook refuses mesh pipelines and rectangle lists; 102 of the 103 refused pipelines of run
  r6 are mesh, so it covers only 1.
* Proposed (recommended) route: flat inputs for vertices 1 and 2 as in item 4, produced by the mesh stage.

## Recommendation (fidelity first)

1. **Route A, translator lowering to per-primitive flat inputs (mesh pipelines; all Apple GPU families).** Values are bit
   copies, weights come from the hardware `SV_Barycentrics`, vertex order is the index order (verified below).
   Cost, recorded only: per primitive 8 bytes per component read from vertices 1 and 2 (median 80 B, p90 176 B, max
   304 B); at 64 primitives median 5 KB, max 19 KB; at 128 primitives median 10 KB, max 38 KB, so the largest of them
   exceed the 32 KB mesh limit (S-M rows: 520 mesh programs are already over it).
2. **Route B, native `vertex_value` on Apple10 (the vertex pipeline and, later, mesh pipelines too).** Exact, no extra
   output, but needs MSC to emit it or a bitcode writer AGX accepts, and is M5+ only. Re-test with every MSC release
   (the names are already inside the dylib).
3. Vertex pipelines (1 of the 103 refused pipelines): old #300 geometry-shader route or Route B; not prototyped here.
Not recommended: any MSC switch (none exists), relying on D3DMetal (it refuses), DXC-based rewrite in the product.

## Prototype

`metal/back/tests/gfx_attr_vertex.rs` (new file, nothing else in the repository changed):
* `the_three_vertex_values_reach_the_pixel_stage_as_flat_inputs`: HLSL MS (groupshared exchange of six vertex values,
  `SetMeshOutputCounts`, two primitives with index orders (0,1,2) and (4,5,3), per-primitive flat `c1`,`c2`, per-vertex
  flat `c0`) and a PS weighting `c0*b.x + c1*b.y + c2*b.z` with `SV_Barycentrics`; drawn through the wire/MSC/Metal 4,
  read back, compared with the CPU barycentric evaluation (3/255 tolerance, 176+ painted pixels, edges excluded).
* `the_first_value_is_the_provoking_vertex_of_the_index_order`: `c0` equals vertex 0 for the first and vertex 4 (not
  the last or the next one) for the rotated primitive.
Result: both pass on arm64 and `--target x86_64-apple-darwin`; a deliberately swapped-weights variant fails
(mutation check); `rustfmt --check` and `cargo clippy -p victory-metal-back --test gfx_attr_vertex -- -D warnings` clean.
Run with `VICTORY_HELPER_DXC=/tmp/dxc-d3dmetal DXC_LIB=<D3DMetal libdxcompiler.dylib>` (without them: SKIPPED).

## The three offline proofs

1. Unit test: the two tests above.
2. Census (PS kind, `victory-census convert --kind ps`, census branch build 51a3f286, same machine):
   before: 4853 refused by MSC (4852 attributeAtVertex + 1 i64 atomic). After the pixel-half lowering of the 4852:
   MSC accepts **4852 / 4852**; full Metal pipeline build **4721 / 4852** (one flat input per component row, unpacked,
   gave 4499). The 131 others fail in `DXC failed`: the harness builds the partner vertex shader with one float4 output
   per input element and DXC refuses more than 31 non-system elements (all 131 have 32..38; none reached Metal).
   The partner shader is a generic full-screen triangle, so this proves acceptance and build, not pixels.
   Rows: `research/attr-vertex/census-after-ps.jsonl`.
3. Frame replay: not rerun. The f300 corpus (`research/captures/2026-11-27-d3dmetal-world`, intro card, 8 pipelines)
   has no attributeAtVertex pipeline (the two earlier Metal replays under `research/captures/replays/` contain no
   refusal), and this branch changes no product file, so its pixels are unchanged by construction. A frame with
   attributeAtVertex draws needs a capture of a mesh scene (the r6 run had 103 such pipelines) and the translator change.

## Shared-layer changes (not made; for the lead to escalate)

1. `crates/victory-shader/src/emit/lower.rs` (`vertex_input` ~10838, `per_vertex` ~10781, `eval_channel` ~10800): when the
   host says it has no `attributeAtVertex`, vertex 0 -> `loadInput` of the existing flat input, vertices 1 and 2 ->
   `loadInput` of packed float4 flat elements (reserved semantic, e.g. TEXCOORD 64+n); `emit/shader.rs:559` and `io.rs`
   (`Interp`) declare them as constant-interpolated inputs.
2. The producing stage (mesh, `emit/` mesh path): groupshared array of the vertex values read, barrier at the end of
   the body (uniform control flow), per primitive write `c1 = v[idx.y]`, `c2 = v[idx.z]` as per-primitive outputs.
   Needs the pixel shader's demand when the mesh stage is translated (as `needs_vertex_deltas` did in 4ad2628d).
3. `crates/victory-gpu/src/pipeline/host.rs` (hook) and the host capability set: `attribute_at_vertex = false` on the
   Metal back, true on Windows; the Windows output stays unchanged.
Risks: the barrier in a translated mesh body with non-uniform flow; the 32 KB mesh limit for the largest 5% of shaders;
SV_Barycentrics mode (centroid is refused by MSC); vertex order on strips/fans of a vertex pipeline vs the console is
unconfirmed (an Xbox comparison is pending in issue #203). Rough effort: pixel half 2-3 days, mesh exchange 4-6 days with
tests, vertex pipeline 2-3 days, a captured-frame check 1-2 days.

## Translation matrix rows (S-G-002 and C-B-011)

* disposition: `emulated` (was `stubbed`).
* deviation: "the pixel program receives the three vertex values as flat inputs and weights them with the hardware
  SV_Barycentrics; the values are bit-identical copies and the order is the primitive's index order. Differences: two
  extra per-primitive outputs of 8 bytes per component read (median 80 B, max 304 B per primitive) against the 32 KB mesh
  limit; centroid barycentrics stay refused by MSC; the vertex (non-mesh) pipeline is not covered by this route."
* restore_plan: "可行 (pixel half 4852/4852 through MSC, 4721/4852 through a Metal pipeline, mesh+pixel pair drawn
  against a CPU reference, 2026-11-27): translator lowering as above, shared layer. Highest fidelity and no extra output on
  Apple10 (M5+): `vertex_value<T>` (MSL 4.1, Feature Set Tables 2026-10-23), 待验证: MSC does not emit it and AIR patching
  needs a bitcode writer AGX accepts. Cost is recorded, not a criterion."
