# M4 render: passes, draws, mesh dispatch, graphics state, geometry shaders

Branch `m4-render`, 2026-11-27. What was built on top of `CREATE_GRAPHICS_PIPELINE` (m3-gfx): the
render commands of `proto-render-draft.md`, the front that derives passes from a D3D12 list, the
back's Metal 4 render encoder, and the geometry-shader emulation the game needs (244 refused
pipelines before). Deviations from the draft are listed in section 6.

## 1. Proto

All new commands are fixed-size, version 1, in `proto/src/render.rs` (`docs/metal/proto.md` has the
generated table). Opcodes in the ranges the draft reserved:

| Opcode | Command | Payload |
| --- | --- | --- |
| 0x0501 | `SET_VIEWPORTS` | count (≤ 16) + 16 `Viewport` |
| 0x0502 | `SET_SCISSORS` | count (≤ 16) + 16 `Rect` |
| 0x0503 | `SET_BLEND_FACTOR` | 4 floats |
| 0x0504 | `SET_STENCIL_REF` | u32 |
| 0x0505 | `SET_PRIMITIVE_TOPOLOGY` | `Topology` (point/line/triangle lists and strips) |
| 0x0506 | `SET_INDEX_BUFFER` | GPU address, size, `IndexFormat` |
| 0x0507 | `SET_VERTEX_BUFFERS` | start slot, count, 16 × {address, size, stride} |
| 0x0510 | `DRAW` | `DrawInstanced` arguments |
| 0x0511 | `DRAW_INDEXED` | `DrawIndexedInstanced` arguments |
| 0x0512 | `DISPATCH_MESH` | x, y, z |
| 0x0520 | `CLEAR_ATTACHMENT` | target (0..7, or 8 = depth-stencil), flags, ≤ 8 rects, colour, depth, stencil |
| 0x0600 | `BEGIN_RENDER_PASS` | ≤ 8 colour attachments + depth-stencil, each with view (resource, format, mip, slices) and load action |
| 0x0601 | `END_RENDER_PASS` | store action of every attachment |

`check()` refuses NaN in every float field (a NaN would break the roundtrip). `GraphicsPsoDesc`
grew `gs: StageCode` (after `ms`, 16 bytes; the struct is 2072 bytes); `PsoKind::Geometry` is
`vs + gs (+ ps)`. Fingerprint of v3 is re-pinned in `proto/tests/version.rs`.

## 2. Front: pass derivation and merge rules

`front/src/render.rs` (module docs have the same text). D3D12 has no passes; the front derives them.

* **Begin is lazy**: the first draw, mesh dispatch or clear that needs a pass. `OMSetRenderTargets`
  only changes what is bound (applications set targets, set state and change their minds).
* A **full clear of a bound view before the first draw** becomes that attachment's load action
  `CLEAR`. With rectangles, or after the first draw, it is `CLEAR_ATTACHMENT` inside the pass. A
  clear of a view that is not bound is a pass of its own.
* **End** is `OMSetRenderTargets` with a different set (the same set again is nothing), a
  transition barrier on an attachment, any command that is not render state or a barrier (copy,
  dispatch, UAV clear, indirect dispatch), and `Close`.
* **Store** is `STORE`. `DiscardResource` of an attachment of the pass that has just ended turns
  that attachment's store into `DONT_CARE` by patching the recorded `END_RENDER_PASS`; a discarded
  resource also loads as `DONT_CARE` in the next pass.
* Same render-target set bound twice in a row, with only state in between, is one pass: this is the
  merge the draft asks for, and it is why a frame with hundreds of draws into few targets is few
  encoders (good for TBDR: attachments stay in tile memory between draws).
* State commands (viewports, scissors, blend factor, stencil ref, topology, IB, VB, root
  arguments) are recorded as they come; the back keeps the shadow.

## 3. Back: the Metal 4 render encoder

`back/src/draw.rs` (+ `exec/render_cmds.rs`). One pass may be several encoders: the encoder is made by
the first draw or clear, and a barrier that makes the pass wait for its own fragment work, or a copy or
dispatch in the middle, ends it; the next draw starts a new encoder for the same pass with every load
action `Load` and the state shadow re-applied. The one argument table is set for the vertex, fragment,
object and mesh stages. Bind points: 0 heap, 1 sampler heap, 2 top-level argument buffer, 3 the same buffer for the hull and domain stages of the converter runtime, 4 `IRRuntimeDrawParams`, 5 uniforms (`IRRuntimeDrawInfo`, or the index type for ordinary
draws), 6 + slot vertex buffers (dynamic stride). Depth-stencil state objects, cull, fill, winding,
bias and clip are applied when the pipeline changes. Index type values are the runtime header's
`IRMetalIndexToIRIndex` (Metal's index type + 1: 1 for 16-bit, 2 for 32-bit). Barriers across render and
compute encoders follow the TBDR limits of `barrier.rs`.

## 4. Geometry shaders

The game uses them (244 refused pipelines in the first run); `victory-shader` needs one for the
flat inputs (`GetAttributeAtVertex` is refused by MSC) and for rect lists. They are emulated with MSC's
geometry/tessellation emulation, as in the converter's runtime (`IRRuntimeNewGeometryEmulationPipeline`,
`IRRuntime4DrawPrimitivesGeometryEmulation`):

* Front: `GraphicsPsoDesc.gs` is carried; hull, domain and stream output are still `E_NOTIMPL`.
* Conversion (`back/src/msc/compile.rs`, `ffi.rs`): the VS is converted with
  `IRCompilerEnableGeometryAndTessellationEmulation` and a separate stage-in function
  (`IRMetalLibSynthesizeStageInFunction` from the pipeline's input layout; the object stage cannot use
  Metal vertex fetch) giving `<entry>.dxil_irconverter_object_shader`; the GS gives a mesh function
  and `IRGSInfo` (input primitive, max input primitives per mesh threadgroup, payload). The PS is
  unchanged.
* Pipeline (`render.rs`, `Kind::Geometry`): an `MTL4MeshRenderPipelineDescriptor` with the VS as object
  function (constant `tessellationEnabled` = false), the stage-in function statically linked to the
  object stage, the GS as mesh function (`vertex_shader_output_size_fc` = the VS's output bytes).
* Draw (`draw.rs::draw_geometry`): a mesh draw. Object threadgroups cover the vertices (stride =
  primitives per object threadgroup × vertices per primitive, clamped by the payload and 256 threads),
  mesh threadgroups the GS's input primitives. `IRRuntimeDrawInfo` goes to the uniforms point, the
  vertex buffers as one `IRRuntimeVertexBuffers` block (31 × {address, length, stride}) at point 6.
  The object stage ignores the start index, so it is folded into the index buffer address.
* Topology and GS must agree (a GS that reads triangles with a line list is refused). Adjacency
  topologies are not in the proto yet.

Tests: `back/tests/gfx_draw.rs` (`a_geometry_shader_rewrites_the_triangles_it_is_given`,
`..._can_make_more_primitives_than_it_reads`, `indexed_draws_and_several_primitives_go_through_a_geometry_shader`)
on arm64 and Rosetta.

## 5. Mesh pipelines and the other refusals the game showed

* Mesh pipelines were refused as `E_NOTIMPL`: the cause was the back refusing conservative
  rasterization (the front was fine). Conservative rasterization is now **accepted and ignored**
  (Metal has none; edge pixels may be lost). Known difference.
* A pixel shader that writes more render targets than the pipeline declares: the extra writes are
  **dropped**, as D3D12 does, not refused.
* `OMSetDepthBounds` is accepted and has no effect (Metal has no depth bounds test). Known
  difference; the victory-gpu `depth_bounds` test passes on ours.

## 6. Differences from the draft, known gaps

* `ResolveSubresource` is not turned into a resolve yet.
* Partial (rectangle) clears of integer render targets are refused by the back.
* Layered rendering into 3D textures is limited to one depth plane per pass.
* Hull/domain shaders and stream output: not carried (E_NOTIMPL).
* `ExecuteIndirect` with a vertex buffer or index buffer view in the record, and with a geometry
  shader pipeline: refused (section 7).
* The `early_depth` victory-shader test is registered as a test bug in
  `conformance/expected-fail.txt`.

## 7. ExecuteIndirect of draws and mesh dispatches

Signatures ending in `DRAW`, `DRAW_INDEXED` or `DISPATCH_MESH` (the game: one `DISPATCH_MESH`, stride
12, no count buffer, through victory-gpu's argument clamp) run on the render encoder.

* **Front** (`front/src/indirect.rs`): a signature with a graphics action begins the pass as a direct
  draw does and keeps it open; a compute dispatch ends it.
* **Back** (`back/src/indirect.rs`, `draw.rs::execute_indirect_draws`): the records are Metal's own
  argument layouts (`DrawPrimitives`, `DrawIndexedPrimitives`, mesh threadgroups have the same words in
  the same order as D3D12's), so Metal's indirect draws read them.
  * Mesh dispatch, no count buffer, no root changes: `drawMeshThreadgroupsWithIndirectBuffer` straight
    from the application's buffer, one per record, no extra encoder switch.
  * Anything else goes through the compute expansion kernel of m4-compute (extended: action words,
    output record size, index type). It clamps to the count buffer (dead records become zero-count
    draws), copies the root arguments each record changes into a per-record argument buffer, and for a
    vertex draw writes the converter's runtime parameters (`IRRuntimeDrawParams` = the record's words,
    the index type at +32 bytes), which a converted vertex shader reads and an indirect draw cannot
    provide otherwise. Then one indirect draw per record with the argument table pointed at the
    record's expansion. The kernel is a compute dispatch, so the render encoder ends and the next draw
    continues the pass (load action `Load`), as for any copy or dispatch inside a pass.
  * Why not an `MTLIndirectCommandBuffer`, as for compute: the pipelines MSC makes are not built for
    ICBs, a command in an ICB would rebind the top-level argument buffer per command anyway, and the
    kernel keeps every pipeline as it is.
* Refused, `UNSUPPORTED` (before anything runs): vertex or index buffer views in a record (the table's
  vertex addresses are written by the CPU; Metal's indirect indexed draw takes the index buffer at
  encode time), ray dispatch; `UNSUPPORTED` at run time: a geometry-shader pipeline (the draw maths of
  the emulation need the counts on the CPU). A graphics action with only a compute pipeline bound (or
  the reverse) is `INVALID_ARG`.
* Argument and count buffers are in the residency set (the buffer table); the list keeps them alive.
* Tests: `gfx_draw.rs` `indirect_draws_run_the_records_a_count_buffer_allows`,
  `indirect_indexed_draws_use_the_index_buffer_of_the_list`,
  `an_indirect_mesh_dispatch_reads_its_groups_and_the_count_buffer` (arm64 and Rosetta);
  `front/tests/render.rs` for the pass rules.
* A bug found on the way: a graphics draw compared the pipeline's root signature to the bound one by
  object identity; two `CreateRootSignature` calls on the same bytes are compatible in D3D12 (m4-compute
  fixed that for dispatch), now for draws too.

## 8. Passes without attachments

A draw with no colour target and no depth-stencil (a pixel shader that only writes through views)
used to fail the list. `BeginRenderPass` now carries `width` and `height` (`PROTO_VERSION` 4, opcode
version 2, payload 336 bytes): both are required (1..=16384) when the pass has no attachment, and must
be 0 when it has one. The front sizes such a pass from the largest area the viewports and scissor
rectangles set so far reach; the back sets `renderTargetWidth/Height` and a raster sample count of 1 on
the pass descriptor. Test: `gfx_draw.rs::a_pass_without_attachments_runs_pixel_shaders_that_only_write_through_views`.

## 9. Known gaps after the first full game run

* ExecuteIndirect with a geometry-shader pipeline: `UNSUPPORTED` (the emulation's draw maths need the
  counts on the CPU; an indirect version would run the emulation's compute pass per record). The game
  does this about 1000 times in a 400-second run.
* `COPY_RESOURCE` of a texture onto itself: `INVALID_ARG` (28 times).
* Frame rate with the loading art on screen: 2-3 fps at ~3300 draws per frame.
