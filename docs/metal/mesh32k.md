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
| a'. Also drop outputs the pixel shader has no input for (needs the real PS) | Done in code (`mesh_fit::retry` second try, with `desc.ps`), proven on a synthetic shader; the census has no MS/PS pairing, so its effect on the remaining 143 is not counted (see below). Lossless: nothing reads them. |
| a''. MSC compression options / f32 to f16 | MSC 4.0.3 has none. f32 to f16 would change values: not lossless, not done. |
| b. Split over several threadgroups or an object shader | Impossible as such: the indices are arbitrary inside the 128 vertices, so no vertex subset closes over its triangles; the translator uses the declared maxima as counts, so the real counts are dynamic. |
| c. Compute emulation (MS as a compute writing vertex and primitive buffers, then an indexed draw, like MSC's GS emulation) | The last resort for what a and a' do not take (fidelity ok; cost: a compute pass, a buffer of up to 128 x 336 bytes per group and a second pass; hooks into `render.rs`, which other hands edit). Not built. |
| d. Tighten the declared maxima to the real output | Not possible: the maxima are in DXIL, but the counts are dynamic (`mesh_prologue` in `victory-shader` uses the declared maxima as counts). |

The root cause is in the emitter: the translator numbers the output elements
in signature order, and Metal lays the struct out in that order. A pass in
`victory-shader` (Windows line, annotate-only here) that numbers them by
descending alignment would give the same result without editing bitcode at
run time: **proposed to the emitter owner**; until then the back repacks.

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

## The 143 left, next

* Pairing each refused MS with the PS the game uses (the game's pipeline
  archive; not in the census) lets `retry` drop unread outputs: 4-36 bytes per
  vertex are needed, i.e. 1-9 `float4` outputs of the shaders. This is already
  in the hook; the count is only known with the real pairing (a game run or the
  pipeline archive). Needs the first live run, no extra code.
* Then option c for what remains (record the cost: one compute pass plus
  128 x stride bytes of buffer per mesh group, a second draw).

## Matrix row (C-B-012 / S-M)

> Mesh pipeline over Metal's 32 KB total mesh size: **limit measured and pinned**
> (`round_up(NV,4) x vertex_stride + NP x prim_stride <= 32768`, the driver
> refuses at pipeline creation; no document states it). **520 -> 143** of the
> census (377 rescued losslessly by packing the output struct,
> `back/src/mesh_fit.rs`, hook on refusal only). Remaining 143: pixel-shader
> aware pruning (in place, needs the real MS/PS pair), then compute
> emulation. Proofs: `back/tests/{mesh_limit,mesh_fit}.rs`, census `meshfit` +
> `convert --dxil-dir`, replay byte-identical to the baseline. Emitter-side fix
> proposed (number output elements by alignment). Status: partly done.
