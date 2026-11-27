# Mesh pipelines over Metal's 32 KB (C-B-012 / S-M)

Checked 2026-11-27 on an Apple M5 Max, macOS 27.2.1, SDK 27.2, Metal Shader
Converter 4.0.3, MTL4Compiler. Upstream issue: pragyaangaur/VIctory-macOS#132.

## What the limit is

`MTL4Compiler` refuses a mesh pipeline with
`Total mesh size (N) exceeds the maximum mesh size allowed (32768)`.

* **Who says it**: the GPU driver (AGXMetalG17G) when the pipeline is made.
  The shader library of the same source compiles; Metal Shader Converter has
  no option for it and does not check. (`mesh_limit.rs`
  `it_is_the_driver_at_pipeline_creation_that_refuses`.)
* **What is counted** (measured, pinned in `back/tests/mesh_limit.rs`, model in
  `back/src/mesh_size.rs`):
  `total = round_up(max_vertices, 4) * vertex_stride + max_primitives * primitive_stride <= 32768`
  (inclusive). The **declared** maxima count, not what the shader writes.
  Indices and the topology are not counted. The object-to-mesh payload (16 KB)
  and `threadgroup` memory (32 KB) are separate limits.
* **The stride**: the vertex is a struct. Clip/cull distances first (4 bytes
  each), then the position (16), then every non-position element in the order
  of the **element id** (the distances appear a second time). Each member is
  aligned to its own size (`float3` to 12, `half4` to 8), nothing is added at
  the end. Metal does not drop outputs the pixel shader does not read.
* **Documents**: Metal Feature Set Tables (2026-11-27) and the Metal Shading
  Language spec 4.1 state no total-mesh-size limit; MSC 4.0.3's manual and
  headers have no option. WWDC22 10162's "16 KB" is stale. The number is only
  known from measurement.

## The 520

8,765 translated mesh shaders; 520 are refused (`victory-data/census/convert-ms.jsonl`;
all over by at most 10,240 bytes). All 520 declare 128 vertices, 128
primitives, 128 threads, no group-shared memory, no primitive outputs.

| refused total | 33280 | 33792 | 34816 | 35328 | 35840 | 36352 | 36864 | 38400 | 38912 | 39424 | 39936 | 40960 | 43008 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| shaders | 6 | 51 | 96 | 23 | 47 | 34 | 149 | 26 | 20 | 39 | 3 | 20 | 6 |
| bytes per vertex | 260 | 264 | 272 | 276 | 280 | 284 | 288 | 300 | 304 | 308 | 312 | 320 | 336 |

The model of `mesh_size.rs` reproduces Metal's number for 520/520 (and has no
case where Metal builds what the model refuses, or the other way round).
`victory-census meshstat` writes the per-shader facts
(`victory-data/census/meshstat.tsv`). The DXIL limit is the same 32 KB but
counts 4 bytes per component with no padding, which is why the translator
passes them: a 256-byte DXIL vertex can be 264+ bytes in Metal.

The game's log of the livetr run (`research/game-runs/20261127-metal-livetr-r6`)
has the refusals of 34816 and 35840 bytes.

## What was tried (fidelity first, performance only recorded)

| option | verdict |
|---|---|
| a. **Renumber the output element ids** so the struct has the least padding (the DXIL's declared maxima and every value written stay) | **Done. Rescues 377 of 520, lossless.** 8-64 bytes per vertex of padding go. |
| a'. Also drop outputs the pixel shader has no input for (needs the real PS) | Done in code (`mesh_fit::retry` second try, with `desc.ps`), proven on a synthetic shader; with the real pairs of the title's pipeline book it saves nothing for the 143 (below). Lossless: nothing reads them. |
| a''. MSC compression options / f32 to f16 | MSC 4.0.3 has none. f32 to f16 would change values: not lossless, not done. |
| b. Split over several threadgroups or an object shader | Impossible as such: the indices are arbitrary inside the 128 vertices, so no vertex subset closes over its triangles; the translator uses the declared maxima as counts, so the real counts are dynamic. |
| c. Compute emulation (MS as a compute writing vertex and primitive buffers, then an indexed draw, like MSC's GS emulation) | The last resort for what a and a' do not take (fidelity ok; cost: a compute pass, a buffer of up to 128 x 336 bytes per group and a second pass; hooks into `render.rs`, which other hands edit). Not built. |
| d. Tighten the declared maxima to the real output | Not possible: the maxima are in DXIL, but the counts are dynamic (`mesh_prologue` in `victory-shader` uses the declared maxima as counts). |

The root cause is in the emitter: the translator numbers the output elements
in signature order, and Metal lays the struct out in that order. A pass in
`victory-shader` (Windows line, annotate-only here) that numbers them by
descending alignment would give the same result without editing bitcode at
run time: **done as mac S2** (branch `mac/mesh-order`); the back's repack stays as the fallback.

## The implementation (back only, new files)

* `back/src/bitcode.rs`: LLVM bitstream reader/writer (abbreviations,
  BLOCKINFO, blob, canonical VBR). It reproduces the DXIL streams of all 8,765
  census mesh shaders byte for byte.
* `back/src/mesh_fit.rs`: `repack_outputs(container, consumer)` renumbers the
  elements (metadata id field, the `i32` element operand of each
  `dx.op.storeVertexOutput` call, the element list order), removes the stores
  of the outputs a given pixel shader does not read, fixes the module symbol
  table offsets, rewrites the DXIL part sizes. The container hash is not
  checked by MSC. Anything not understood is an error and the shader is left
  alone. `mesh_fit::retry` is the hook: `exec/gfx.rs create_graphics_pipeline`
  calls it **only after Metal refused a mesh pipeline with "Total mesh size"**
  (`render.rs` is untouched), so every pipeline that builds today is unchanged.
* `back/src/mesh_size.rs`: the size model and `packed_order`.
* `census`: `meshstat`, `meshfit` (`--hashes`, `--dxil-out`), and `convert --hashes
  FILE --dxil-dir DIR` to run the converter on repacked containers.

## Offline proofs

1. **Unit / node tests**:
   * `back/tests/mesh_limit.rs` pins the measured limit.
   * `back/tests/mesh_fit.rs` (synthetic HLSL through DXC, no game data):
     the alternating `float`/`float3` order (276 B/vertex, 35328) is refused by
     Metal with the number; after `retry` the pipeline is made; through the
     executor the picture equals a CPU rasterisation and equals the shader
     written in packed order; 20 `float3` outputs (264 B, nothing to reorder)
     are drawn only after the unread ones are dropped.
   * `back/src/bitcode.rs` tests: round trip, a wider value moves what follows,
     broken streams are refused.
2. **Census, before and after** (the converter on the 520 DXIL, `convert ms`):

   | | refused for Total mesh size | builds |
   |---|---|---|
   | before | 520 | 0 |
   | after `repack_outputs(.., None)` | **143** | **377** |

   **520 -> 143.** The 143 left are over by 512-4,600 bytes (4 B/vertex or less
   needs dropping for 45 of them; 8: 38; 12: 27; 16: 13; 20: 14; 28: 3; 36: 3)
   and are real content at the translator's default pixel shader. Regression
   check: 600 shaders that built before and are repacked anyway (random sample,
   seed 7) all still build (600/600, `convert-ms_fit3.jsonl`); the whole
   population: 5,758 of 8,765 are repackable, the stream is reproduced exactly
   for 8,765. The hook does not touch them: it runs only after a refusal.
3. **Frame-capture replay**: `tools/metal/replay.sh` of
   `research/captures/2026-11-27-d3dmetal-world/rec-a-f300` on this tree
   (`research/captures/replays/20261127-mesh32k-r2`, prefix
   `research/metal-play-prefix-mesh32k`, a copy of the idle `-fix` prefix):
   `replay.png` is byte-identical (md5 458e2f66...) to the previous Metal replay
   of the same capture (`20261127-f300-metal-2`), whose difference to the
   D3DMetal reference is the recorded baseline (78.2 dB, 2,730 of 921,600
   pixels). No regression. Note the capture's 8 pipelines hold no refused mesh
   shader (it is the intro card), so it proves the hook does not disturb a
   frame; the mesh-over-32 KB proof is the synthetic picture test.

## The 143 left (second round, 2026-11-28)

**The real pairs.** The title's pipeline book (`VICTORY_PIPELINE_BOOK=1`,
`known-pipelines.bin` / `known-shaders.bin`, format in
`crates/victory-gpu/src/pipeline/known.rs`, read only; the drops hand's record run,
`victory-data-drops-warm/cache/pipelines`: 83,563 pipelines, 9,887 distinct
(mesh, pixel) pairs, no amplification stage among them) gives the pixel shader each
mesh shader is used with. `victory-census mspair` builds each pair in Metal with the
real pixel shader: as translated, packed, and packed with the unread outputs dropped.

* Of the 520, **441 are in the book** (652 pairs), **79 are not** (the title does not
  create them in that run: not used where it went, as far as the book says).
* Of the 143 left after packing: **116 are in the book, 27 are not**.
* **Dropping what the pixel shader does not read saves 0 bytes for all 116**: the real
  pixel shaders read every output these meshes write (and every component: trimming
  unread trailing components saves 0 too). The stride after pruning equals the stride
  after packing (260 to 292 bytes). So the pixel-shader-aware pruning is correct and
  stays in the hook (it matters for shaders whose pixel shader reads less), but it
  **rescues none of the 143**.
* 71 of the 116 are also refused by MSC (`dx.op.attributeAtVertex`, the other node):
  fixing the size alone would not draw them.
* The 143 are over by 512 to 4,608 bytes in total, 4 to 36 bytes a vertex: real
  content.

**S2 (translator numbers the outputs by alignment)**, branch `mac/mesh-order`
(`docs/mac/PATCHES.md` S2): `victory-shader/src/emit/mesh_order.rs`, 6 lines in `emit/mod.rs`.
The census on the translator with S2: **520 -> 143, the same 377 as the runtime repack,
no regression among the other 8,245 (0 of them stop building)**, `convert ms` 8,622 of
8,765 build. The runtime repack stays as the fallback (a translation made before S2, a
cache of translations).

**What is left needs the compute emulation** (option c). Plan, not built:

1. *The kernel.* A mesh shader is a compute-like kernel: the same MSL function body can
   run as a compute function over `groups x threads` that writes `vertices[g * NV + v]`
   and `primitives[g * NP + p]` and the counts to buffers. MSC cannot make this from a
   mesh DXIL (it emits `[[mesh]]`), so the conversion must be our own: translate the
   mesh DXIL to a compute DXIL in `victory-shader` (the output stores become stores to
   a UAV: `storeVertexOutput` / `storePrimitiveOutput` / `setMeshOutputCounts` /
   `emitIndices` are the only mesh-specific ops, all in `lower.rs`) and give it to MSC as
   a compute shader. This is the larger part: a new lowering mode of the translator
   (Windows line shared layer, now ours to change after the 23:42 decision).
2. *The draw.* After the compute dispatch (a barrier from compute to render in the same
   command buffer), a vertex-only render pipeline (vertex fetch from the buffers by
   `vertex_id`, the real pixel shader unchanged) draws indexed with the written
   indices. The vertex shader is a tiny generated one: it reads the vertex struct from
   the buffer and writes the pixel shader's inputs, so the pixel shader and its
   interpolation need no change. Counts are dynamic (the mesh sets them), so the draw is
   `drawIndexedPrimitives` with an indirect buffer written by the compute pass (the
   count of primitives x 3), per group: `MTLIndirectCommandBuffer`-free, one indirect
   draw per group (`m5-gs.md` already does this for geometry draws: the expansion
   kernel writes the record).
3. *Changes in `render.rs` / `draw.rs`* (the other hands' files, so to be agreed):
   `PreparedGraphics` gets a third kind, `MeshAsCompute`, holding the compute pipeline
   and the vertex pipeline; `create_graphics_pipeline` falls to it when `mesh_fit::retry`
   fails; `dispatch_mesh` for that kind encodes compute + barrier + indirect draws
   instead of `drawMeshThreadgroups`; the vertex/primitive buffers come from a
   per-queue scratch ring (`NV x stride x groups`, at most 128 x 336 B = 43 KB a group;
   a dispatch of 10,000 groups is 430 MB, so the groups are chunked, as the indirect
   path does in 256s). Amplification-stage pipelines would need the object shader run
   first (not among the 143: the book has none).
4. *Cost (recorded, not weighed)*: one compute pass and one render pass per `DispatchMesh`
   instead of one mesh draw (the render encoder is ended and resumed with `Load`, as for
   the indirect draws), a write and a read of 33 to 43 KB per group through memory, no
   mesh-shader culling of unused vertices. Ballpark: 2 to 3 times the cost of a native
   mesh draw for these meshes; the 143 are 1.6 percent of the mesh shaders (the 116 in
   the book: 1.3 percent), and 71 of those 116 need `attributeAtVertex` first.
5. *Tests*: the synthetic 20-`float3` shader of `mesh_fit.rs` with all outputs read (so
   nothing can be pruned): the emulated draw equals the CPU picture and the native draw
   of a mesh with 120 vertices declared.

**Recommendation**: do `attributeAtVertex` (the node behind 71 of the 116 and the bigger
loss: 22 to 26 percent of the title's draws) before building the emulation; the size
problem is then 45 pipelines in the book (0.5 percent of the mesh shaders).

## Matrix row (C-B-012 / S-M)

> Mesh pipeline over Metal's 32 KB total mesh size: **limit measured and pinned**
> (`round_up(NV,4) x vertex_stride + NP x prim_stride <= 32768`, the driver
> refuses at pipeline creation; no document states it). **520 -> 143** of the
> census (377 rescued losslessly by packing the output struct: `mac S2`
> in the translator, and `back/src/mesh_fit.rs` as the fallback hook on
> refusal). Remaining 143 (116 used by the title, 27 not): pruning for the
> real pixel shaders saves nothing; they need the compute emulation (plan
> above, not built). Proofs: `back/tests/{mesh_limit,mesh_fit}.rs`, census `meshfit` +
> `convert --dxil-dir`, replay byte-identical to the baseline. Status: partly done.
