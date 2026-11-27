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

## Implementation design (mac S1, branch `mac/attr-vertex`, base `fork/mac/livetr` 12289486)

References are `git show fork/mac/livetr:<file>` line numbers at 12289486. The implementation lives on the Mac patch
branch (shared layer); this branch keeps the tests, the census and this document. Patch number **S1**
(`docs/mac/PATCHES.md`), every hook line in an upstream file carries the comment `mac S1`.

### Capability and how it travels

* Name: `attribute_at_vertex`. Windows true (default, output unchanged), Metal back and D3DMetal false.
* `crates/victory-shader/src/emit/lower.rs` next to `host_settings()` (1130): `set_host_attribute_at_vertex(bool)` /
  `host_attribute_at_vertex()` (a static `AtomicU8`: unset, yes, no; unset falls back on `VICTORY_ATTRIBUTE_AT_VERTEX=0`,
  which the census and the metal tests use because they translate without a victory-gpu host). Re-exported at `emit/mod.rs:124`.
* `victory-gpu/src/mac/attribute_at_vertex.rs` (`host_has_it()`: `!rosetta()`, env override `VICTORY_ATTRIBUTE_AT_VERTEX=0|1`);
  called once from `victory-gpu/src/host.rs` at host creation, beside `set_host_rdna` (985), before any shader is translated.
* The shader cache and the live translator library key on it: `host_settings()` (lower.rs:1130, `[u32; 4]`) carries it as
  bit 31 of the last word (the RDNA word); `shader_cache.rs::build()` already hashes `host_settings`;
  `victory-shader-dylib/src/lib.rs:57` decodes it (`host[3] & 0x7FFF_FFFF`, `host[3] >> 31 == 0`).

### Pixel half (`emit/lower.rs`, `emit/vertex_flat.rs`)

* New `emit/vertex_flat.rs`: `VertexRead { name, index, channel, vertices }`; `canonical`, `slot`, `elements`, `encode/decode`,
  `pixel_reads(io, inputs)`; `with_reads`/`current` (the demand a mesh translation is given), `ask`/`covers` (see below).
* `Lower::new` (lower.rs:1403, `s.inputs = x.inputs.clone()`): for a pixel program on a host without the capability, the reads
  are `pixel_reads` over `io.interps` (`io.rs:129` `Interp::{Interpolated, Weighted, Vertex}`; a row is read per vertex when
  `per_vertex` 10781 says so) and the added elements are appended to `s.inputs`.
* `vertex_input` (10840-10870, opcode 137): with the capability false, vertex 0 -> `load_any_input` (2657, `loadInput` opcode 4)
  of the row's own input (constant, the provoking vertex), vertices 1 and 2 -> `loadInput` of the added element (bitcast
  where the input is float: the elements are u32 so the bits pass untouched). `flags::BARYCENTRICS` stays set (10847).
  `per_vertex` (10781) and `eval_channel` (10800) are untouched: they only call `vertex_input`.
* A channel the prescan missed (16-bit packed inputs name other channels than the instruction does: the first build left
  4456 of 4852 failing with "a per-vertex value the layout lacks") makes `vertex_input` ask for it and fail with
  `vertex_flat::MORE`; `lower_counted` (135) lowers again with everything asked for (at most 8 rounds).
* `emit/shader.rs:559-575` (the constant mode fix-up of unused elements of a register) needs no change: the added
  elements take rows of their own, from 32 up.
* Element naming/packing: semantic `TEXCOORD`, indices 64, 65, ...; a title's own TEXCOORD indices are
  not expected near 64; a collision would show as a duplicate semantic in the signature. Four scalars to an element in the canonical order (name, index, channel; vertex 1 before 2),
  the last element short; DXIL component type 5 (u32); constant interpolation (mode 1); rows from
  `max(existing rows) + 1` and at least 32 (`vertex_flat::first_row`). The census measured 38 scalars at most
  (median 10, p90 22, p99 29): at most 10 elements.
* **32-element cap**: DXIL/Metal cap varying elements by scalars, not elements. Metal: 124 varying scalars per stage
  (MSC/AIR limit "Mesh unique scalar count of N exceeds limit of 124" / "Number of varying components(N) exceeds the
  limit (124)"). The census partner (a VS) had a separate 31 non-system-element limit in DXC itself: that is why 131 programs
  were stuck (see "The 131"). Real mesh stages do not have that limit; the 124 scalars are real: 20 programs of the title
  exceed it once the added scalars count (see acceptance). Not solved here; a pixel program's own 124-scalar inputs + the
  added ones. (A later patch can drop channels read at only one vertex position by sharing,
  or reuse the title's own unused columns.)

### Mesh half (`emit/lower.rs`)

* The demand: `HostShader::vertex_reads` (new field, `emit/mod.rs:407`; VSH1 bytes after the views, `emit/wire.rs`,
  decode accepts none), `emit::translate_with(container, &[VertexRead])` (`translate` = no reads; `emit/mod.rs:467`).
  Sources of truth: the pixel shader's `vertex_reads`; the mesh stage is translated with them (a thread-local
  `vertex_flat::with_reads`, no change of the `Parts` literals in the tests).
* `Lower::new` MS block (1585-1635, after `s.lds_dwords = ms.lds_bytes.div_ceil(4)` at 1590): group memory grows by
  `vertices * stride + primitives` dwords (stride = read channels; vertices/primitives = min(declared maximum, lanes)),
  refused ("per-vertex exchange passes group memory") above 32 KiB; the added per-primitive elements are appended to `s.prims`
  *before* `mesh_prologue` (4767) so its `mesh_output_bytes` check (`container.rs:119-133`, `lower.rs` ~4899) counts them.
* `export()` (12279-12565): the vertex store loop (`storeVertexOutput`, 171, at 12454): when the output element and
  channel are demanded, the exact bits are also stored to `lds[base + lane * stride + read]` inside the same guard
  (`vtx_store`). 16-bit vertex outputs read per vertex are refused.
* `emit_indices` (12516): the packed indices (10 bits each) go to `lds[corners + lane]` (`vtx_corners`); `vtx_prologue`
  zeroes it for every lane first (a primitive nothing exports is culled with indices 0).
* Barrier placement under non-uniform control flow: lanes leave the program at `LaneExit::End` (12663, `Term::Ret`) and at
  `abort_block` (12727): both now branch to one lazily made epilogue block (`leave`, `vtx_epilogue`), which every lane reaches
  after the body's divergence has ended, runs `group_sync` (`meeting_mode()` 931) and then lane n reads `lds[corners+n]`,
  loads vertices 1 and 2 of its primitive from the vertex area and writes `storePrimitiveOutput` (172). So the barrier is
  outside any divergent branch, and no body barrier is moved (the body's own `s_barrier`s keep `no_barrier`, 1635).
  The epilogue is filled at the end of `run()` before `fill_phis` (2330).
* 32 KiB mesh budget: `mesh_output_bytes` = 4 B x scalars x (maximum rounded to 32); the added per-primitive scalars
  cost 4 B x D x 128 per mesh (all 8765 mesh programs of the title have 128 primitives): +512 B per scalar read. Failure
  behaviour is the existing one at 4899: when the outputs do not fit, the maxima come down to the lanes (no change here:
  they are already 128), and with `VICTORY_MESH_OUTPUT_STRICT` the translation is refused; Metal refuses a pipeline
  over 32 KiB at pipeline build (the 520 already over). That line is the subject of the mesh 32 KiB work (S2),
  not S1; the numbers are below.
* Centroid: not touched. `SV_Barycentrics` mode comes from `Barycentric` of the input VGPRs (`barycentric` 2751); MSC refuses
  centroid barycentrics today, so a pixel program using centroid keeps failing for that reason.

### Vertex pipelines (second commit)

Three ways to feed a vertex-pipeline pixel shader vertices 1 and 2 were weighed:

1. *`nointerpolation` + provoking vertex only*: gives vertex 0 (the provoking vertex) and nothing else. Not enough.
2. *`SV_VertexID` + a buffer*: the pixel stage would need the vertex stage's outputs in a buffer written by the same draw;
   there is no ordering between a draw's vertex writes and its fragment reads. Not faithful.
3. *Geometry stage* (**chosen**): a geometry shader sees the whole triangle in the API's order, the back already runs it as
   an object+mesh pair (`docs/metal/m5-gs.md`), the title's rectangle lists use the same slot, and `emit/rect.rs` is the template.

`victory_shader::emit::vertex_pipeline_gs(vs, ps, reads)` (`vertex_flat.rs`) makes a `gs_6_6` container: triangles in, one
triangle out, every emitted vertex carries the vertex shader's outputs (the pixel shader's constant elements declared
constant) and the added constant elements with vertices 1 and 2. In `victory-gpu/src/pipeline/host.rs` (hooks near 723,
the rectangle stage's code; the `RECTANGLES` case with such a pixel shader gets no pipeline) it takes the place of
`rect_gs`, and if the host refuses it there is no pipeline (nothing writes the pixel shader's inputs). Strip and fan: the
geometry shader's triangle comes in the order D3D defines for the topology, i.e. the order the console's rasteriser gives
`attributeAtVertex` its vertex ids; the provoking vertex is the first emitted. The Xbox comparison for strips
(issue #203) is still open; the title's draws are all triangle lists (m5-gs.md).
HLSL equivalent on the GPU: `gfx_attr_vertex.rs::a_geometry_stage_hands_the_other_vertices_...` (VS + GS + PS, compared with
the CPU barycentric evaluation, list order, second primitive's provoking vertex = its first).

### The 131

The census builds a pixel shader's partner VS from its input signature. DXC refuses (and aborts on) a VS with more than 31
non-system outputs, which stopped the 131 programs with 32-38 inputs before Metal. `metal/census/src/dxc.rs::mesh_for` now
makes a one-triangle `ms_6_6` partner for them (unvalidated: DXC's validator also limits vertex+primitive rows to 32; the
added TEXCOORD>=64 elements are per-primitive outputs as in the real mesh stage; exact component widths), chosen by
`convert::graphics` when the VS partner fails (the row says `partner:"ms"`). With the S1 translator (packing four scalars to
an element) 195 programs use it (the layout is different from the research rewrite's). `convert --only FILE` runs a hash list.

### Commit split (branch `mac/attr-vertex`) and tests

1. `mac: S1 per-vertex values as flat inputs, mesh path`: capability, `vertex_flat.rs`, pixel half, mesh exchange, wire,
   `HostShader` field, victory-gpu pipeline/cache hooks. Tests (victory-shader, `emit/vertex_flat_tests.rs`, `vertex_flat.rs`):
   capability true keeps `attributeAtVertex`; false emits none and reports the reads; settings differ by bit 31; mesh with
   demand stores the primitive outputs and adds the element; both stages agree on layout; exchange over 32 KiB refused;
   wire round trip.
2. `mac: S1 the capability defaults from VICTORY_ATTRIBUTE_AT_VERTEX` (census and tests).
3. `mac: S1 a per-vertex read the pixel prescan missed restarts the lowering` (the 4456 -> 0 fix).
4. `mac: S1 vertex pipelines: a geometry stage...`: `vertex_pipeline_gs` + `HostPipeline::new` hook; test: the container's signatures.
5. `docs/mac/PATCHES.md` S1 entry.

## Acceptance (commands, expected numbers)

Environment for all: `. tools/metal/env.sh; export RUSTUP_TOOLCHAIN=1.98.1;
DXC_LIB=<D3DMetal.framework>/Versions/A/Resources/libdxcompiler.dylib VICTORY_HELPER_DXC=/tmp/dxc-d3dmetal`.

1. Translator unit tests and GPU pixel tests.
   * `cargo test -p victory-shader -p victory-gpu --locked` on `mac/attr-vertex` (numbers in the S1 entry of `docs/mac/PATCHES.md`; victory-shader was 209 passed before the patch).
   * `cargo test -p victory-metal-back --test gfx_attr_vertex`: three tests (mesh path, provoking vertex, VS + GS + PS) against the
     CPU reference.
2. Census, capability false, full corpus (scratch worktree = `attr` + `mac/attr-vertex`, `VICTORY_ATTRIBUTE_AT_VERTEX=0`):
   ```
   victory-census translate --game <game> --out <dir> --jobs 4          # 25985 containers: 25756 ok, 229 failed; the same 229 as before the patch (no pixel program among them)
   victory-census convert --kind ps --out <dir> --only <4852 hashes> --jobs 4
   ```
   or `cargo test -p victory-metal-census --release --test attr_vertex_corpus -- --ignored --nocapture` with
   `VICTORY_GAME_DIR`, `VICTORY_ATTR_PS_LIST`, `VICTORY_ATTR_REQUIRE=1`.
   Result (2026-11-28): translation: 0 attributeAtVertex; of 4853 programs (4852 + the one `atomicBinOp.i64`) **4832 build
   through MSC and a Metal pipeline, 21 fail: 20 on Metal's 124 varying scalars (126-127 with the added elements), 1 the
   unrelated `atomicBinOp.i64`**. Rejected for attributeAtVertex: **4852 -> 0**; not buildable: 20 (the scalar limit).
   Before (this branch's research): 4852 refused by MSC.
3. Mesh newly over 32 KiB. There is no pairing of the title's pixel and mesh programs (names do not tell), so the count is
   by assumption: `meshstat.tsv` (8765 mesh programs, all 128 vertices and 128 primitives, 520 already over 32 KiB:
   Metal refuses them) plus `4 x D x 128` bytes for D scalars read at vertices 1 and 2 (D = the pixel shader's
   demand: median 10, p90 22, p99 29, max 38):

   | D (scalars read from vertices 1/2) | mesh programs newly over 32 KiB (of the 8245 under it) |
   |---|---|
   | 4 | 324 |
   | 10 (median) | 853 |
   | 22 (p90) | 2362 |
   | 38 (max) | 4504 |

   The group-memory exchange ((vertices x D + primitives) dwords) stays under 32 KiB for every mesh program up to D = 38 (3 pass it at D = 50).
   These are listed separately for the mesh 32 KiB work; S1 does not decide what to do with them (the translator's
   behaviour when the outputs do not fit is the existing one).
4. Replay (`tools/metal/replay.sh`) of the capture corpus: see the results at the end of this section.

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
