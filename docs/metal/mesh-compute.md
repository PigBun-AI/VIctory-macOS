# Mesh shaders as compute simulation (P0: the 55 percent)

Written 2026-11-28 on an Apple M5 Max, macOS 27.2.1, Metal Shader Converter 4.0.3,
branch `ms-compute` (from `fork/metal-livetr` 0cf85e13). Design, estimate and a small
prototype. Nothing on the main line changed except one `pub mod` line; no game was
started, no replay was run.

## 1. The problem and the answer

Of the 4,834 real (mesh, pixel) pairs (translator with S1 + S2, census `s1pair`,
2026-11-28) only 2,170 (44.9 percent) build in Metal. The other 2,660 are:

| Cause | Pairs | Share |
|---|---|---|
| Total mesh size over 32 KiB (`round_up(NV,4) x vertex_stride + NP x prim_stride`) | 1,261 | 26.1 % |
| Apple's GPU compiler crashes in `AGCModule<AGX::Impl::MeshProgramKey>::compileLLVMObject` (about 0.5 s, then `XPC_ERROR_CONNECTION_INTERRUPTED`) | 1,400 | 29.0 % |
| over the varying limit | 3 | 0.1 % |

Both causes live inside Metal's **mesh pipeline**: the 32 KB limit is the driver's
check of that pipeline, the crash is in its compile. A compute kernel that writes the
vertices and primitives to memory, followed by an ordinary vertex + fragment draw that
fetches them, touches neither.

**Recommendation (one sentence):** make the translator (`victory-shader`) emit, for
each problem mesh shader, a compute kernel (the mesh body with its output stores
turned into stores to a scratch UAV) and a tiny vertex-fetch shader, and let the back
run `Dispatch` + barrier + non-indexed `Draw` with the unchanged S1 pixel shader, for
the problem pairs only (about 2,660 pairs, 55 percent), as the fallback after the
existing refusal and crash-denylist hooks. Estimate 42 person-days (29 to 63); the
main risk is the back runtime (compute cannot run inside a render pass).

## 2. Which layer: MSC, translator, or back (MSC 4.0.3, checked 2026-11-28)

**MSC has nothing for this.** `/usr/local/include/metal_irconverter.h`,
`metal_irconverter_runtime.h` (IR_VERSION 4.0.3) and the manual
(`/opt/metal-shaderconverter/docs/UserManual.md`) were read:

* No option converts a mesh (or object) shader into a compute function, and no
  option emulates mesh/object stages. Mesh DXIL always becomes `[[mesh]]`.
* `IRCompilerEnableGeometryAndTessellationEmulation` and the `IRRuntime*GeometryEmulation`
  helpers go the **other way** (geometry and tessellation shaders become mesh shaders;
  `m5-gs.md`).
* Relevant limits it does state: `SV_ViewID` is unsupported, `GetAttributeAtVertex` is
  unsupported (S1 exists for this), `SV_CullPrimitive` is supported, wave size must be
  32. I found no official release notes for the 4.0 final (only third-party text), so
  "nothing in 4.0.3" is from headers and manual, not from a changelog.
* But MSC converts a **compute** DXIL (UAV stores, `rawBufferStore`) fine. The prototype
  proves it (section 5).

So the conversion is ours. Three places could do it:

| | A. D3D12-level emulation in the front (the title's `DispatchMesh` becomes a compute dispatch + draw before the back sees it) | B. Back rewrites the mesh DXIL bitstream | C. Translator emits compute DXIL + fetch VS (**recommended**) |
|---|---|---|---|
| Correctness | needs the front to know the mesh shader's output layout, which it does not (it holds DXIL only after translation) | `bitcode.rs` reads and writes a bitstream but never inserts instructions; the rewrite replaces about 6 intrinsic kinds and adds a resource, so it needs an IR editor | the translator already walks these exact ops (`lower.rs`: `setMeshOutputCounts` 168, `emitIndices` 169, `storeVertexOutput` 171, `storePrimitiveOutput` 172) and knows the signature and interpolation; a lowering mode is natural |
| Work | large, wrong place; breaks the layering (front must not know Metal limits) | 10+ days for the editor alone, then a rewrite | 9 days (kernel 6, vertex fetch 3) |
| Performance | same | same | same; the translator can also prune what the pixel shader does not read |
| Relation to S1/S2 | none | none | shares the output-signature analysis with S2 (`mesh_order.rs`); S1's per-primitive flat attributes are just more primitive outputs |

The prototype's text-level lowering (`tools/metal/ms2cs.py`, DXC's disassembler ->
rewrite -> DXC's assembler) shows the rewrite is mechanical, about 430 lines of
Python. In production it belongs in the translator, which emits DXIL itself and needs
no text round trip. Windows-line shared code is ours to change since the 23:42
decision (see `mesh32k.md`).

**S1 and S2.** S1 (`vertex_flat.rs`, branch `mac/attr-vertex`) is still needed: MSC
refuses `GetAttributeAtVertex` in the pixel shader whichever stage feeds it, so the
pixel shader must be the S1 pixel shader. In the emulation S1's extra outputs are
per-primitive uint4 records (`TEXCOORD64/65...`), which the compute kernel stores and
the fetch VS passes on as flat values. S2 (output numbering by alignment) is not needed
for an emulated shader (our scratch layout is our own, packed to 4 bytes), but it stays
as the cheap path for the 377 pairs it rescues: those keep Metal's native mesh path,
which is cheaper than the emulation.

## 3. Design

### 3.1 Shaders

For a mesh shader with `NT` threads, `NV`/`NP` declared maxima:

* **Kernel** (`cs_6_6`, same `numthreads`, `SV_GroupID` / `SV_GroupIndex` unchanged).
  `setMeshOutputCounts(v, p)` stores `v, p` to the group's header. `storeVertexOutput`
  and `storePrimitiveOutput` become `rawBufferStore` into the group's slot of a root
  UAV at a reserved register (`u0, space 4000000000`, appended as the **last** root
  parameter, so TLAB offsets of the application's parameters are unchanged and the
  root-argument shadow is reusable). `emitIndices` packs `v0 | v1<<8 | v2<<16` into the
  primitive record. `SV_CullPrimitive` is a bool stored as an i32. All the mesh body
  (resource loads through the heap, wave ops, groupshared) is kept as is.
* **Scratch layout per group** (`gstride` bytes, built by the translator, passed to the
  back as a `vmcl 1` text blob; `back/src/mesh_compute.rs::Layout` parses it):
  `[counts 16 B][NV x vertex_stride][NP x prim_stride]`, elements 4-byte aligned, no
  Metal padding rules.
* **Vertex fetch shader** (`vs_6_6`): `SV_VertexID = 3 x primitive + corner`,
  `SV_InstanceID = group`. It reads the primitive's packed indices and the vertex record
  and returns exactly the pixel shader's inputs with the interpolation modes of the
  mesh's output signature (DXIL interp codes 1 nointerpolation, 2 linear, 3 centroid,
  4 noperspective, ...). A primitive past the group's count, or culled, gets
  `SV_Position = (0,0,0,0)`: a degenerate triangle, discarded by the rasterizer.
  Per-primitive outputs are flat, so the provoking vertex does not matter (and they
  stay constant even if the corner order differs).
* **Draw**: `Draw(vertex_count = 3 x NP, instance_count = groups)` per `DispatchMesh`
  (x, y, z flattened). No indirect draw is needed, because dead primitives are
  degenerate. This costs vertex-stage work for the declared maximum, not the actual
  count (128 prims in all sampled shaders).

Pixel shader, depth, blend and render targets are unchanged: the PS is the same object
as in the native path.

### 3.2 Back runtime (new file, minimal hooks)

* `PreparedGraphics` gets a third kind `MeshAsCompute { cs, vs_ps, layout }` (compute
  pipeline plus a vertex pipeline). Created in `create_graphics_pipeline_inner` when the
  mesh pipeline is refused for size (`mesh_fit::retry` fails) **or** the pair is on the
  AGX crash denylist (`msc/guard.rs`, `CRASHED_PAIRS`, `mesh-pair-denylist.txt`). Same
  precedent as the `mesh_fit::retry` hook. For crashing pairs the guard already knows
  the pair, so no 0.5 s crash is paid at all.
* Root signature: the application's plus one trailing root descriptor (UAV for the
  kernel, SRV for VS/PS).
* `dispatch_mesh` for that kind encodes: scratch allocation, `SetRootDescriptor`,
  compute dispatch of `groups`, barrier (dispatch -> vertex), `Draw`.
* **Scratch ring**: `groups x gstride` bytes (median 45 KB, max 66 KB a group in the
  sample, section 5.3). Allocated from a per-frame ring; if a dispatch needs more than
  the ring segment, chunk the groups (as `indirect.rs` already chunks by 256).
* `ExecuteIndirect` with `DispatchMesh`: group counts are on the GPU. Needs the scratch
  bound by the maximum count in the argument buffer (D3D12 gives it), then an indirect
  compute dispatch and an indirect draw (`instance_count` read from the same record);
  `m5-gs.md` already has the pattern.

### 3.3 The hard part: compute cannot run inside a render pass

Metal 4's render encoder has no dispatch. D3D12 lets the title interleave
`DispatchMesh` and `Draw` freely inside one render pass. Options:

1. **Pre-scan** the command block from `BeginRenderPass` to `EndRenderPass`; if a
   `SetPipeline` names an emulated pipeline, hoist every such kernel dispatch into a
   compute pass **before** the render pass (the scratch regions are disjoint, one per
   draw), then encode the render pass. Cost: one extra compute encoder per pass with an
   emulated draw; no render-pass split. **Risk:** hoisting is only valid if the kernel
   reads nothing the earlier draws of the same pass write (UAV writes from pixel
   shaders, or a render target sampled as a texture). D3D12 itself forbids render-target
   reads inside a pass, but pixel-shader UAV writes feeding a later mesh shader are
   legal. Needs a check (sample the replays) and a fallback: split the pass at that
   draw.
2. **Split the pass at every emulated draw**: correct, but every split stores and loads
   all attachments. At about 1,800 mesh draws a frame this is not affordable; it is only
   the fallback.

Option 1 is the recommendation; its pre-scan is the main back risk.

### 3.4 What is not handled (explicitly)

* Amplification (object) stage: `ms2cs.py` refuses payloads (tested). The book has none
  among the problem meshes (`mesh32k.md`).
* `SV_ViewID`: unsupported by MSC too.
* More than 256 vertices: D3D12's maximum is 256, so 8-bit packed indices suffice.
* Meshes with `NV x vertex_stride` in scratch for 10,000 groups are 450 MB; the ring
  chunks.

## 4. Cost

### 4.1 What a group moves (200 real meshes, section 5.3)

Written per group (counts + NV x vertex_stride + NP x prim_stride, all 128/128/128
threads): **median 45 KB** (crash pairs 47 KB, size pairs 45 KB, mean 44 KB; max 66
KB). The vertex stage then reads each corner's record: of the same order, so about
**90 KB per group** of traffic, about 1.5x less if the translator prunes outputs the
PS does not read (not measured, `mesh32k.md` found the real pixel shaders read all of
them for the 116 size pairs, so I do not rely on it).

### 4.2 Per frame (scenarios, not measurements)

The group counts per `DispatchMesh` have **not been measured** anywhere in the repo or
docs (replays are locked until 08:00 and the captured `frame.bin` is a different
container format that I did not decode; first measurement on the list in 6). Budget
facts: `xbox-frame-budget.md`: M5 Max 614 GB/s vs Series X 560 GB/s (1.10x), about
11 to 13 GB a frame sustainable at 60 fps (18.7 GB peak); about 1,800 mesh draws a frame.

| emulated groups per frame | traffic at 90 KB a group | share of 12 GB | compute+draw time at the measured rate (bench below) |
|---|---|---|---|
| 5,000 | 0.45 GB | 4 % | about 1 ms |
| 20,000 | 1.8 GB | 15 % | about 3 to 4 ms |
| 100,000 | 9 GB | 75 % | does not fit |

Emulating only the problem pairs (55 percent), not all mesh draws, keeps this
proportional to their groups.

### 4.3 Measured on this machine (bench test)

`VICTORY_MC_BENCH=<groups>` runs the synthetic shader (64 triangles, scratch 36 KB a
group; the native comparison is a *small* 6 KB-output mesh doing the same picture, so
the native side is the best case). Median of 7 submissions (includes submit/wait
overhead of about 0.5 ms):

| groups | scratch | native small mesh | emulated (compute + draw) | compute + tiny draw |
|---|---|---|---|---|
| 1,024 | 37 MB | 0.46 ms | 0.52 ms | 0.48 ms |
| 4,096 | 134 MB (earlier layout) | 0.67 | 1.35 | 0.82 |
| 8,192 | 294 MB | 0.53 | 1.91 | 1.31 |
| 16,384 | 537 MB (earlier layout) | 0.97 | 2.62 | 1.76 |
| 32,768 | 1.07 GB (earlier layout) | 0.87 | 4.65 | 2.67 |

Rough rate: the kernel writes about 400 GB/s at 32k groups; the emulated draw costs
about 2 to 5 times native at these sizes, and beyond 1,000 groups it is
bandwidth-bound as computed in 4.2. The "earlier layout" rows were run before one extra
flat output (`idv`) was added to the synthetic mesh (36 KB a group instead of 32.8 KB).
Small draws (fewer than 1,000 groups, the likely common case) are within a
millisecond of native.

## 5. Prototype

### 5.1 What exists (all in the branch)

* `tools/metal/ms2cs.py`: mesh DXIL text (from `dxcdis`) -> compute DXIL text +
  `vmcl 1` layout. Handles resource-less meshes, bool `SV_CullPrimitive`, primitive
  outputs, S1's uint4 primitive outputs; refuses payloads.
* `tools/metal/dxc-text-tools.sh`: builds `dxcdis` / `dxcasm` (x86_64, D3DMetal's
  DXC) from `research/attr-vertex/agx-crash/*.cpp` (C++ is not on the asset policy's
  allowlist, so they are not copied).
* `metal/back/src/mesh_compute.rs`: `Layout` (parse, check, `scratch_bytes`,
  `draw_vertices`), 2 unit tests. Pure; builds anywhere.
* `metal/back/tests/mesh_compute.rs`: 6 tests through the real executor on the GPU.

### 5.2 Results

Run: `. tools/metal/env.sh; tools/metal/dxc-text-tools.sh /tmp/mc/tools; export VICTORY_DXC_TEXT_TOOLS=/tmp/mc/tools; cd metal; nice -n 15 cargo test -j 4 -p victory-metal-back --test mesh_compute -- --nocapture`

| Test | Result |
|---|---|
| `metal_refuses_the_big_mesh` | synthetic mesh (192 vertices of 160 B by D3D12's count, Metal's padding makes it over 32,768; DXC accepts it) is refused by the driver with `Total mesh size`. Note this one the repack would rescue; it is the shape, not the size class, that matters for the next test |
| `the_compute_form_draws_what_metal_draws_on_the_small_mesh` | **passes**: 2 groups x 64 triangles (every 7th culled by `SV_CullPrimitive`, per-primitive tint, a flat per-vertex uint to check the provoking vertex): the emulated picture equals Metal's own mesh path on the small mesh within one level on all 2,304 pixels (660 lit); the scratch is checked on the CPU (counts, packed indices, cull flag, position) |
| `the_lowering_refuses_what_it_does_not_handle` | passes (payload refused) |
| `real_pair_builds_where_the_mesh_pipeline_crashes` (env `VICTORY_MC_REAL_MS`, `_PS`) | **passes** for mesh 7d3d2b08c42b24b4 (S1+S2 translation) + pixel b6e7a1d41b618738: the mesh pipeline crashes Apple's compiler (359 ms, then the XPC error); the compute pipeline builds in **142 ms** and the vertex + pixel pipeline in **27 ms**. Root signature of the translator's convention (16 dwords user data, two root SRVs in space 1) plus the trailing UAV/SRV. Build only: no real draw, as the mesh needs the game's heaps |
| `real_compute_forms_build_in_apples_compiler` (env `VICTORY_MC_REAL_METALLIBS`) | **200 of 200** compute forms build (sample below); the vertex forms **199 of 200** (the one failure is the DXC front end itself crashing on a vertex shader with more than 32 output rows, not Metal; production does not go through DXC for the VS) |
| `bench_emulated_against_native` (env `VICTORY_MC_BENCH`) | numbers in 4.3 |

### 5.3 The 200-mesh sample

A random sample of S1+S2 meshes from the real census (seed 11): 120 AGX-crash pairs,
60 size pairs, 20 that build natively (outside the repository; dumped with the
read-only `victory-census s1pair` of `worktrees/attr-s1d`, `S1PAIR_LIST`/`S1PAIR_DUMP`).
All 200 lowered, assembled and passed `metal-shaderconverter`, and all built as
compute pipelines in the GPU compiler (0.09 s each on average: 200 in 18 s the first time, 20 to 50 ms
when cached). Layout statistics: all are 128 threads,
128 vertices, 128 primitives; group scratch min 22.5 KB, median 45 KB, p90 57 KB, max
65.5 KB; vertex stride median 192 B; only float, uint (flat) and bool scalars appear.

### 5.4 What the prototype does not prove

* A real draw of a real pair: it needs the game's heaps; not possible without a replay
  (locked). The build proof and the synthetic picture proof are separate; the product of
  both is not tested.
* The pass pre-scan (3.3), indirect dispatch, scratch ring chunking, and
  `DispatchMesh` with y, z > 1.
* The text round trip is for the prototype only (DXC's disassembler and assembler
  limit the round trip; one 32-row VS crashed DXC).
* Interpolation of linear/centroid/noperspective/sample varyings is mapped but only
  tested for linear + flat; `SV_Position` noperspective is carried by the VS.

## 6. Work estimate (person-days)

| Module | Work | Days (range) | Confidence |
|---|---|---|---|
| **Translator** (`victory-shader`) | mesh -> compute lowering mode (ops above; layout; cull; payload refusal; wave/group-shared kept) | 6 (4 to 9) | high: the prototype lowers 200/200 real meshes in text form; production is the same rewrite on the translator's own IR |
| | vertex-fetch shader emitter (interpolation modes, flat/integer, bool, `SV_Position`, clip/cull distance semantics) | 3 (2 to 4) | high |
| | prune to the pixel shader's inputs, S1 interplay | 2 (1 to 4) | medium |
| **Back runtime** | `MeshAsCompute` kind, root-sig append, hook after refusal/denylist, scratch ring, `dispatch_mesh` | 6 (4 to 8) | medium |
| | render-pass pre-scan and hoisting, UAV-dependency check, split fallback | 6 (4 to 10) | **low**: the main risk |
| | `ExecuteIndirect` + `DispatchMesh` (GPU counts, indirect compute and draw) | 4 (3 to 6) | medium |
| **Tests** | back: synthetic picture, UAV in mesh, wave, heap resources, indirect; conformance rows | 4 (3 to 5) | high |
| **Census coverage** | `s1pair` emulation column: lower + build all 2,660 pairs; denylist update, regression of the 2,170 that build natively | 3 (2 to 4) | high |
| **Performance** | measure real group counts per draw (replay of a world capture), prune, pack to 4 B / half, ring sizing, decide which pairs stay native | 5 (3 to 10) | **low**: the numbers in 4.2 are scenarios until measured |
| Buffer | integration with `render.rs` / `draw.rs` owners, review | 3 | |
| **Total** | | **42 (29 to 63)**: translator 11, back 16, tests + census 7, performance 5, buffer 3 | |

Confident: the shader conversion (translator), tests, census. Risky: the pass
pre-scan (back), real group counts and bandwidth (performance), amplification stage
(not needed by the book; extra 5+ days if it ever is).

## 7. Order of work and open questions

1. Measure per-draw group counts and the share of problem pairs per frame (first
   replay after the 08:00 lock) before the back runtime; if the emulated share is
   above 20,000 groups a frame, prune first.
2. Translator lowering + census proof for the 2,660 pairs (no game, 9 to 11 days).
3. Back runtime with the pass pre-scan (12 to 16 days), then `ExecuteIndirect`.
4. Decide with the owner of `render.rs` / `draw.rs` how the pre-scan hooks in (the
   `mesh_fit::retry` call in `exec/gfx.rs` is the precedent for the pipeline side).

Open: does the title's pixel shader UAV output feed later mesh shaders in one pass?
(decides 3.3 option 1 versus the split). Not yet checked in a replay.

## 8. Interface v1 (接口 v1: translator <-> back)

Owner: the back-runtime hand (branch `mc-back`, 2026-11-28). The prototype
(`ms2cs.py`, `back/src/mesh_compute.rs`, `tests/mesh_compute.rs`) is the reference where
this section is silent; where it disagrees, **this section wins** and the prototype is
changed to match. A change request from the translator side goes to the back-runtime
hand, who decides. Versions: the layout text is `vmcl 1`, the container part is `VMCS`
version 1; a reader that meets another version refuses the emulation (the pair stays
refused, as before), it never guesses.

### 8.1 What travels, and how (delivery)

Between victory-gpu (L6) and the back the only channel is the shader bytes of the D3D12
pipeline description. So the three products travel **inside the mesh shader's own DXIL
container** as one extra container part, FourCC `VMCS`, appended by the translator:

```text
VMCS part payload (little endian, every offset from the start of the payload, 4-aligned)
   0  u32 version            = 1
   4  u32 flags              = 0 (reserved, must be 0)
   8  u32 layout_off, u32 layout_len   the `vmcl 1` text (8.3), UTF-8, no NUL
  16  u32 cs_off,     u32 cs_len       a complete DXIL container: the compute form (8.4)
  24  u32 vs_off,     u32 vs_len       a complete DXIL container: the vertex-fetch shader (8.5)
  32  data
```

* The container stays a valid mesh container: the `DXIL`, `SFI0`, `PSV0`, ... parts are the
  native mesh shader, unchanged, so the native path (and `mesh_fit`, which edits the
  `DXIL` part in place) works as it does today. The converter ignores unknown parts (as it
  ignores the hash, `mesh_fit.rs`); the back strips nothing.
* The translator recomputes the container hash over the final bytes if it signs at all.
* The part is optional. No `VMCS`: the back behaves as before the emulation (the problem
  pair is refused). `VICTORY_METAL_MESH_COMPUTE=0` also ignores it.
* The translator may attach it to every mesh shader it can lower (the back only reads it
  for pairs it must emulate: size refusal after `mesh_fit::retry`, AGX crash cache, pair
  denylist). A shader it cannot lower (payload / object stage, 16- or 64-bit outputs it
  does not widen, more than 256 vertices, non-triangle topology) simply gets no part.
* Both inner containers are ordinary DXIL (`cs_6_6`, `vs_6_6`, entry `main`), valid for
  MSC 4.0.3 on their own, **against the root signature of 8.2**.

### 8.2 Binding convention (root signature)

Let `N` be the number of root parameters of the application's root signature, `space`
and `register` the two numbers of the layout text (`space` 4000000000, `register` 0;
reserved so they cannot collide with the title's registers). The back builds a *second*
root signature for the emulation: the application's, byte for byte (flags, parameters,
ranges, static samplers), plus two parameters **appended last** (so the offsets of the
application's parameters in the top-level argument buffer do not move):

| root index | compute form (8.4) | vertex-fetch VS (8.5) and the pixel shader | what the back puts there |
|---|---|---|---|
| `N`   | root **UAV** `u<register>, space<space>`: the scratch (a `RWByteAddressBuffer`) | root **SRV** `t<register>, space<space>`: the same bytes (a `ByteAddressBuffer`) | GPU address of this dispatch's group 0 (8.6) |
| `N+1` | root **CBV** `b0, space<space>`: 16 bytes `{dim_x, dim_y, dim_z, 0}` | declared (CBV `b0, space<space>`), not read | GPU address of the dispatch dimensions |

Both extended signatures give the **same** top-level argument buffer layout (all new
parameters are 8-byte root descriptors in the same places), so one argument buffer serves
the compute dispatch and the draw; the back checks that and refuses the emulation if MSC
reflects otherwise. Visibility is ALL. The kernel and the VS never see any other
parameter of the extension; the pixel shader is the unchanged S1 pixel shader and does not
read the scratch. The application's heap-indexed flags are kept (the mesh body still reads
`ResourceDescriptorHeap[]`).

### 8.3 Scratch layout (the `vmcl 1` text, as the prototype)

Keys, one per line after the line `vmcl 1`: `threads X Y Z`, `maxv`, `maxp`, `vstride`,
`pstride`, `vhead`, `parea`, `gstride`, `space`, `register`, then one line per element
`v|p NAME INDEX KIND TYPE ROWS COLS OFFSET INTERP` (`KIND` = `DXIL::SemanticKind`, `TYPE`
= `float|uint|int|bool`, `INTERP` = `DXIL::InterpolationMode`). One new optional key:

* `reads MASK` (hex): the DXIL `ResourceKind`s the **compute form** can read through
  heap-indexed or table SRVs, as a bit set (bit `k` = kind `k`: 1 Texture1D, 2 Texture2D,
  3 Texture2DMS, 4 Texture3D, 5 TextureCube, 6 Texture1DArray, 7 Texture2DArray, 10
  TypedBuffer, 11 RawBuffer, 12 StructuredBuffer; bit 0 = "something not listed").
  Absent = all bits. The back uses it only for the pass-split safety rule (8.8); a too
  small mask is a correctness bug of the translator, a too large one only costs splits.

Per group `g` (flat index, 8.4) at byte `g * gstride`:

```text
   0                          u32 vertex count, u32 primitive count, 2 x u32 reserved
   vhead + v * vstride        vertex v: its elements
   parea + p * pstride        primitive p: u32 indices (v0 | v1<<8 | v2<<16), then its elements
   gstride = parea + maxp * pstride
```

* Every component is 4 bytes (`half`/16-bit outputs are widened to `float`; `bool` is a
  `u32`, 0 or 1). All offsets and strides are multiples of 4 (the prototype uses 16;
  translators may pack tighter). `vhead >= 16`, `maxv <= 256`, triangle lists only.
* Elements of a record are in the mesh signature's id order, `OFFSET` is the byte offset
  inside the record (`>= 4` in a primitive record). `SV_Position` is the vertex element
  of kind 3, `SV_CullPrimitive` the primitive element of kind 30 (a `bool` stored as `u32`).
  Every other element is carried to the pixel shader under its own semantic name and index.
  **Per-primitive elements are always flat**; this includes S1's `uint4` records. Per-vertex
  elements keep their interpolation mode from the signature (`INTERP`).
* The scratch is **not cleared**. The kernel writes the counts of every group, even
  when they are zero (`SetMeshOutputCounts` is group-uniform); a primitive `p < count`
  must have its indices written (D3D12 requires it). Whatever else is in the region is
  garbage and is never read.
* A group's scratch is addressed with 32-bit byte offsets in the kernel (`g * gstride`):
  the back keeps one dispatch's scratch under 2 GiB.

### 8.4 The compute form (kernel)

* `cs_6_6`, `numthreads` = the mesh shader's, entry `main`. The body is the mesh body
  (resource loads through the heap, wave operations, groupshared, `SV_GroupIndex`,
  `SV_GroupThreadID`, `SV_GroupID` all unchanged); only these operations change:
  `setMeshOutputCounts(v, p)` -> store `v, p` at the group header; `emitIndices` ->
  store the packed indices; `storeVertexOutput` / `storePrimitiveOutput` -> `rawBufferStore`
  of the 4-byte component at its offset. Output stores need no bounds check (D3D12 makes
  out of range output writes undefined).
* **Group slot.** `g = id.x + dim_x * (id.y + dim_y * id.z)`, `base = g * gstride`, with `id` =
  `SV_GroupID` and `dim_*` from the `b0` block of 8.2 (offsets 0, 4). The prototype
  computes `g = id.x`, which is the same for `y = z = 1`; the dimension block is what
  makes `DispatchMesh(x, y, z)` with `y, z > 1` work and what the back writes in every
  case. A dispatch of zero groups is never issued.
* No resource of the application may be bound at `space` (the back would refuse the pipeline
  with a clear text if MSC reflects a clash).
* An amplification (object) stage, payloads, `SV_ViewID`: not lowered (no `VMCS` part).

### 8.5 The vertex-fetch shader

* `vs_6_6`, entry `main`, **no input layout, no vertex buffers**. Inputs: `SV_VertexID` and
  `SV_InstanceID` only. The back issues one **non-indexed `Draw(3 * maxp, groups, 0, 0)`
  with a triangle-list topology** whatever the title set (a mesh pipeline has none), where
  `groups = dim_x * dim_y * dim_z`:
  `SV_InstanceID` = `g` (the flat group of 8.4), `SV_VertexID` = `3 * p + corner`.
* It reads the scratch (root SRV of 8.2) as a `ByteAddressBuffer`: `base = g * gstride`;
  `dead = p >= prim_count || any index >= vertex_count || cull flag != 0`; a dead primitive
  returns `SV_Position = (0,0,0,0)` and zeros elsewhere (a degenerate triangle: the
  rasterizer drops it). Otherwise it returns, with `corner`'s vertex record `v`: the
  position, every per-vertex element of `v`, every per-primitive element of `p`.
* Its output signature is exactly the signature the pixel shader reads: same semantic
  names, indices, component types, and interpolation modes (per-primitive and integer ones
  `nointerpolation`). Outputs the pixel shader does not read may be dropped (pruning is
  optional; the layout still holds them, the scratch is written in full).
* Flat attributes take the provoking vertex of the triangle, the first corner (D3D12 and
  Metal agree by default); per-primitive values are equal at the three corners, so
  nothing depends on it. `SV_PrimitiveID` in the pixel shader is `p` (the slot), as in the
  native path, per instance.

### 8.6 What the back does (its side of the contract)

1. At pipeline creation of a pair that needs it: reads `VMCS`, parses the layout, builds
   the extended root signature (8.2), converts `cs` against it, converts `vs` + the
   unchanged `ps` against it (topology triangle, the pair's blend/depth/raster state), and
   checks that the thread-group size equals `threads`, that both argument-buffer layouts are
   identical and agree with the application's for parameters `0..N`.
2. Per `DispatchMesh(x, y, z)`: scratch from the back's ring: `x*y*z*gstride` bytes (16-aligned);
   argument buffer = the application's current root arguments (as the shadow holds them
   at that point of the command stream) + the two trailing addresses; compute dispatch of
   `x, y, z` groups; barrier compute -> vertex stage; the draw of 8.5.
3. Per `ExecuteIndirect` with a `DISPATCH_MESH` action: the same on the GPU, one record at a
   time; scratch for the records of one call is one arena sub-allocated on the GPU, a
   record whose groups do not fit gets zero groups (counted, not silent: `ListStats`).
4. Pass structure: all kernels of one front pass are encoded in one compute encoder
   *before* the render encoder (pre-scan); a draw that may read what an earlier draw of
   the same pass wrote through a UAV gets its kernel encoded just before it instead,
   which splits the pass there (8.8).

### 8.7 Failure behaviour

An emulated pair that cannot be built (missing/foreign `VMCS`, MSC refuses the compute
form or the VS, layouts disagree) keeps the old refusal for the pair. A draw whose
scratch cannot be had (ring too small for it) is dropped with a drop-statistics reason.
Nothing here aborts the process.

### 8.8 Safety rule (back internal, listed for the translator's `reads`)

Offline check (`research/mesh-compute-check/REPORT.md`): zero real dependencies of
"UAV written earlier in the front pass, read by a problem mesh". The runtime insurance is
one rule: a problem-mesh draw whose kernel may read (SRV/CBV: root descriptors, bounded
table ranges, and the heap-indexed kinds in `reads`) a resource that an earlier draw of the
same front pass may have written through a UAV is **not** hoisted: its kernel is encoded
right before it (the render encoder ends and the pass continues with `Load` actions),
so the kernel sees the writes. Resources are matched by identity (buffer range / texture);
accesses the back cannot resolve (heap-indexed UAV writes by the pixel shader against
heap-indexed reads by the kernel) are counted as `unproven` and do not split by default
(`VICTORY_METAL_MESH_COMPUTE_SPLIT=unproven` splits on them, for A/B).
