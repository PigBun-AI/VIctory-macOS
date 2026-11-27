# M5: indirect geometry-shader draws, castable block formats, self copies

Branch `m5-gs` (off `metal-backend` bf29821b), 2026-11-27. The three things the first full game run
(`research/game-runs/20261127-200844-metal`) showed as errors: 996 `EXECUTE_INDIRECT` refused
(`an indirect draw with a geometry shader`), 115 `CreateShaderResourceView` answered `E_INVALIDARG`,
28 `COPY_RESOURCE` of a texture onto itself answered `INVALID_ARG`. Each failed `ExecuteCommandLists`
dropped the whole list (about 1000 lists in 400 s), so these were not cosmetic.

No proto change (`PROTO_VERSION` stays 4): all three are decisions of the back and the front.

## 1. ExecuteIndirect of draws through a geometry shader

### What the game does

Measured with the new census line (`VICTORY_METAL_LOG=1` prints each distinct kind of geometry draw once,
`victory-metal back: geometry draw: ...`; `back/src/draw.rs::note_geometry`):

| draw | topology | vertex of the VS output | primitives per mesh threadgroup |
|---|---|---|---|
| direct | triangle list, non-indexed | 16, 32 and 48 bytes | 32 |
| indirect `DRAW` (stride 16) | triangle list | 16 bytes | 32 |
| indirect `DRAW_INDEXED` (stride 20) | triangle list, indexed | 32 bytes | 32 |

No strips, lines, points or adjacency. The geometry shaders are the ones `victory-shader` makes itself: the
rectangle list's completion shader (`emit/rect.rs`: three corners in, the fourth made from them, two triangles
out) and the vertex-delta shader (`emit/deltas.rs`: flat P10/P20 inputs). The topology the title gives is a
triangle list in every case (a rectangle is three vertices); the "rect list" is the shader's business, the
back sees `TRIANGLE_LIST` and a GS that reads triangles. Every `ExecuteIndirect` of the run has
`MaxCommandCount` 1 and no count buffer (996 of 996), so the cost that matters is one record per call.

### Which way: the expansion kernel, not the runtime header

The converter runtime header (`metal_irconverter_runtime.h`, 4.0.3) was read in full for this
(`/usr/local/include/metal_irconverter_runtime/`), and the User Manual
(`/opt/metal-shaderconverter/docs/UserManual.md`) searched: the geometry/tessellation emulation helpers are
`IRRuntime[4]Draw[Indexed]PrimitivesGeometryEmulation` and their `...GSTS...` forms. **All take the counts as
CPU arguments**, and compute the object threadgroup count from them
(`IRRuntimeCalculateObjectTgCountForTessellationAndGeometryEmulation`); there is no indirect form of the
emulation draw, and the plain indirect helpers (`IRRuntimeDrawPrimitives` with an indirect buffer) are for
vertex pipelines. The manual has no section on indirect draws with emulation. So the header cannot be used
as it is.

Chosen: **the m4-compute expansion kernel** (the one that already expands the records for graphics
`ExecuteIndirect`), extended to write, per record, what the emulation's mesh draw needs, then one
`drawMeshThreadgroupsWithIndirectBuffer` per record. Reasons:

* The only things that depend on the GPU-side counts are the object threadgroup count
  (`ceil((count - non_provoking) / vertices_per_object_threadgroup)`) and the `IRRuntimeDrawInfo` instance
  count; everything else of the draw is a function of the pipeline and the topology (the same numbers
  `draw_geometry` computes for a direct draw, now one function, `draw.rs::geometry_plan`, used by both).
  A kernel doing that arithmetic is ten lines; a second compute pass "per record" as the emulation's own
  runtime would need has nothing more to compute.
* It reuses what exists: the clamp to the count buffer, the per-record root argument copies, the 256-record
  chunking, the barrier from the kernel to the draws, the residency of the argument and count buffers.
* It does not depend on an ICB (the converted pipelines are not built for them, see m4-compute).
* The cost per `ExecuteIndirect` is one small compute dispatch (render encoder ends, the pass continues with
  load action `Load`, as for the existing indirect draws) and, per record, one indirect mesh draw. The game's
  records are single, so that is one dispatch and one draw per call.

### The expansion of a geometry record (64 bytes)

| bytes | content |
|---|---|
| 0..20 | `IRRuntimeDrawParams`: count, instances, start vertex (non-indexed) or start index zeroed (indexed: the start index is folded into the index buffer address, the object stage ignores it), base vertex, start instance |
| 20..32 | mesh threadgroup counts `{ceil(...), instances, 1}`, what Metal reads (`GS_GROUPS_AT`); `{0, 0, 0}` for a dead record (past the count buffer, fewer vertices than a primitive, no instances) |
| 32..56 | `IRRuntimeDrawInfo`: the pipeline's fixed part from the CPU (`GeometryPlan::info`), the instance count in the `gsInstanceCount` field and the index buffer address at the record's start index, from the kernel |

The kernel gets the pipeline's plan (`GsExpand`: strides, vertices per primitive, strip overlap, index buffer
address, size and width) in its parameter block (160 bytes, was 96). For an indexed record the count is
**clamped to the indices that remain in the bound index buffer** after the record's start index (nothing else
bounds what the object stage reads), and to nothing if the start is past the end.

### Intermediate memory

The emulation has no intermediate buffer: the vertex stage's output travels in the mesh stage's payload
(16 KB per object threadgroup, a pipeline-state setting, the same as for direct draws). What the indirect
form allocates is the expansion: 64 bytes per record from the constant ring, in chunks of at most 256 records
(`MaxCommandCount` at most 65 536 as for every `ExecuteIndirect`), and one 496-byte vertex buffer block. Both
come from the ring, which is residency-managed by the queue like every other allocation of a list; the
argument and count buffers are kept alive by the list and are in the residency set (the buffer table puts
every buffer there), as are the vertex and index buffers the draw reads. Worst case is therefore bounded by the ring, not
by the draw's vertex counts, and a record's counts cannot make the back allocate: they only decide how many
threadgroups the GPU runs.

### What it does to the picture in the game (open)

First full run with it (`research/game-runs/20261127-205345-metal`): no refused list any more (0
`ExecuteCommandLists failed`, 1021 before), but the window shows the title art at t=40 s and then **black**
in 10 of 10 later captures, where the baseline (`bf29821b`) and the items-2-and-3-only build (`2be5dd1b`)
show the credits text in about half of them. The A/B runs that isolate it (all the same build, one switch
each; captures of the game window only):

| variant | picture |
|---|---|
| the indirect geometry draws as committed | black from t=81 s |
| `VICTORY_METAL_SKIP_GS_INDIRECT=1` (return before the kernel and the draw) | credits shown |
| kernel dispatched, mesh draw not encoded | credits shown |
| kernel dispatched, mesh draw encoded with zero groups (non-indexed; indexed skipped) | credits shown |
| real groups | black |

So the encoder split, the expansion dispatch and the pass resume are not the cause (and
`a_pass_keeps_what_was_drawn_around_indirect_geometry_draws` pins that a pass survives them); something the
real draws render is. No GPU error is reported (`VICTORY_METAL_LOG=1` now prints a failed submission:
`a submission failed: ...`; none). Records seen at the end of a frame, one in 20: `{3, 0, 0, 0}`, `{84, 0|1, 0, 0}`,
`{4, 2034, 0, 0}` (count, instances, start vertex, start instance): mostly empty or one instance. Two causes
are open, and this needs a screen to split: (a) the draws are right and are the title's own fade or
composite, shown black because the compute work that should feed them does not translate (the run logs
about 20 `shader does not translate ... draws are dropped` compute shaders), which the baseline hid by
dropping the whole list; (b) a defect in the real-count path that the unit tests (counts of 3 to 4 vertices, 1
to 2 instances, 300-vertex strips) do not reach. Next step: `VICTORY_METAL_SKIP_GS_INDIRECT=1` against the
default with the screen unlocked, then only the indexed or only the plain draws (a variable like it,
dropped from the build because the screen was locked when the experiment was wanted).

### A bug found on the way

The direct draw sized a triangle strip's object threadgroup as `256 / 3` primitives plus the two overlap
vertices: 86 * 3 + 2 = 260 threads, more than the 256 the pipeline allows, and Metal ran nothing. Not
reachable by the game (no strips with a GS) but now by tests: the plan takes the overlap out of the budget
(`(256 - overlap) / vertices`, and the payload likewise).

### Tests

`back/tests/gfx_draw.rs` (arm64 and Rosetta; they need DXC, `VICTORY_HELPER_DXC` and `DXC_LIB`, else
`SKIPPED`, as every GS test):

* `indirect_draws_through_a_geometry_shader_match_the_direct_ones`: two records, the second with two
  instances and one vertex over a triangle; no count buffer, count 1, count 0. Pixels are compared with the
  direct draws of the same arguments, and with the CPU reference.
* `indirect_indexed_draws_through_a_geometry_shader_match_the_direct_ones_and_stay_in_the_buffer`: start
  index and base vertex; a count of 600 over a 9-index buffer is clamped to the direct draw's pixels.
* `indirect_strip_draws_through_a_geometry_shader_match_the_direct_ones`: a four-vertex strip and a
  300-vertex zigzag (more than one object threadgroup).

## 2. CreateShaderResourceView -> E_INVALIDARG (115)

All 115 were one case, the back's `WRITE_DESCRIPTOR: texture view: a view in BC7_UNORM of a BC5_UNORM
texture (different format families)`: the view was in another **block** format than the texture.
`victory-gpu` makes every colour texture castable (`resource.rs::castable`: every format of the same element,
for block formats every block format of the same block size, so BC7 -> BC2, BC3, BC5, BC6H, BC7 sRGB) and
creates textures of one title memory in different formats on one host texture; a view in another BC format
is then legal on D3D12 (enhanced barriers). The front sent the flag (`CAST`), the back's rule allowed casts
only between uncompressed formats.

Metal does not have it: `newTextureViewWithPixelFormat` of a BC texture is only possible to its own family
(BC1 and BC1 sRGB). Measured with a 30-line Objective-C probe (not in the repository): BC7 as BC5 or BC3,
BC1 as BC4, BC7 as RGBA32Uint, BC1 as RG32Uint all fail with `source texture pixelFormat ... not compatible
with texture view pixelFormat ...`, an assertion that aborts the process; same family works. So block
formats cannot be cast views on this API, and there is nothing to fix at the view.

Done instead, at the creation, which is where `victory-gpu` can handle it:

* The front refuses `CreateCommittedResource3` and `CreatePlacedResource2` with `E_INVALIDARG` when a
  castable list names a block format of another family than the texture's, or mixes block and uncompressed
  formats (`front/src/resource.rs::castable_formats_supported`, called from `shell/device.rs`; the list was
  ignored before, only its length was sent). `victory-gpu` has the fallback for exactly this: "the host
  refuses castable formats: textures of one memory in other formats get their own host texture", and
  placed -> committed -> committed without the list.
* The back's rule is unchanged (block casts refused, now by a test that pins it, `format::tests`), and the
  refusal is a clean error instead of the framework's abort if something reaches it.
* Under `VICTORY_METAL_LOG=1` each refused list is said once, and a refused texture view prints the
  resource and the view description (`CreateShaderResourceView refused: resource ..., view ...`), which is
  how the next one would be found.

Result: 0 failed SRVs in the following run (115 before). The cost is that BC textures are no longer
created through placed resources in a heap but committed (about 20 000 refused creations in 420 s, each
retried by `victory-gpu` as committed without a list): see the game run below.

## 3. COPY_RESOURCE of a texture onto itself (28)

`CopyResource(dst, src)` with `dst == src`: illegal in D3D12. The cause is on `victory-gpu`'s side of
the seam: it gives two title resources of one memory (and one description) **one host texture**, and the
title's copy of one onto the other arrives as a copy of the host texture onto itself. Whole resources, every
subresource, same texture: the copy has no effect. It is not a subresource copy (`CopyTextureRegion` is
where subresources are chosen, and that path is unchanged: a region copy within one texture is allowed when
the regions do not overlap).

So it is accepted as a **no-op** (`back/src/copy.rs::copy_texture_resource` returns before it encodes
anything; `stats.copies` does not change), for both equal Metal formats and the family-cast path. Test:
`texture_gpu.rs::copy_resource_of_a_texture_onto_itself_is_accepted_and_does_nothing` (contents kept); the
old "onto itself" refusal assertion was removed from `copies_with_a_wrong_argument_are_errors_not_silent`.

## 4. BVH builder

`BVH builder held back: not all its pipelines are made frame=1222 pipelines=53 made=50 refused=3`: the same
50/53 before (`20261127-200844-metal`) and after these changes (`20261127-203731-metal`). The cause is in the
shared layer (the three pipelines `victory-gpu` cannot make on the host); not touched.
