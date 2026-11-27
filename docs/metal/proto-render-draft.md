# Render commands: opcode design draft (phase 2, not built)

Status: **draft**, written 2026-11-27 by m3-gfx at the end of phase 1 (pipeline creation). Nothing
here exists in `victory-metal-proto` yet; the numbers sit in the ranges phase 1 reserved
(`0x0500–0x05FF` render state, draws, mesh dispatch; `0x0600–0x06FF` render passes and encoder
bindings; `docs/metal/proto.md` section 5b). Phase 1 gave the front and back what a draw needs to
exist: `CREATE_GRAPHICS_PIPELINE` (0x50), the interpreter's `Pso` that can be graphics
(`PsoBody::Graphics`), and `render::GraphicsPipeline` (Metal pipeline state, a cached
`MTLDepthStencilState`, the rasterizer state, vertex slots, mesh sizes). A `DISPATCH` with a graphics
pipeline bound already answers `INVALID_ARG`.

What the game asks of this: `DrawInstanced` 3 call sites, `DrawIndexedInstanced` 1, `DispatchMesh` 1
(docs/metal/front-api-surface.md); the work is mostly compute and mesh. So the design is sized for
that: a **small** command set, no indirect draws now (0x0900 is ExecuteIndirect, M2), no
tessellation, no geometry stage, no bundles (the front expands `ExecuteBundle`).

## 1. Principles

1. **The front derives the pass, the back runs it.** D3D12 has no render passes (the render-pass API
   is optional and the game uses `OMSetRenderTargets`); Metal cannot do anything outside of one.
   The front already tracks resource states and knows the list, so it turns `OMSetRenderTargets`,
   `ClearRenderTargetView`, `ClearDepthStencilView`, `DiscardResource`, `ResolveSubresource` and
   barriers into explicit `BEGIN_RENDER_PASS` / `END_RENDER_PASS`. The back never guesses.
2. **RTV and DSV descriptors stay in the front.** Today `CREATE_DESC_HEAP` of kind RTV/DSV is an
   "unbacked" heap in the front (docs/metal/m1-exec.md). Keep it so: a render target is not a thing
   the shaders read through a heap. `BEGIN_RENDER_PASS` carries the *resolved* views (texture id,
   format, mip, slice range, dimension) that the front looked up from the CPU descriptor handle. No
   RTV/DSV heap exists in the back, `COPY_DESCRIPTORS` on them is a front-only memmove, and the
   texture object model stays with m3-tex.
3. **State commands are cheap and order-free.** The back keeps a shadow of viewports, scissors,
   blend factor, stencil reference, primitive topology, index buffer and vertex buffers (as the
   compute path keeps `bound` for root arguments), applies it when the encoder exists and before the
   first draw that needs it. So the front may emit `SET_VIEWPORTS` before `BEGIN_RENDER_PASS` (D3D12
   allows that order), and the front dedups same-value sets.
4. **Metal 4 render encoders take no inline state for buffers.** Everything a shader reads is in the
   argument table (same table kind as compute: bind points 0 heap, 1 sampler heap, 2 TLAB, 4 draw
   arguments, 5 draw info, 6+ vertex buffers); the same table object serves all four render stages
   (vertex, fragment, object, mesh) because the argument-buffer layout is verified equal across the
   stages of a pipeline (`prepare_graphics`).
5. **Fixed-size commands**, as every command so far (8-byte header + POD payload, `registry.rs`
   rows and goldens for each).

## 2. Render pass: from D3D12 calls to `MTL4RenderPassDescriptor`

### 2.1 What the front tracks

Per command list, in the front's recorder:

* `bound_rt[8]`, `bound_ds`: the resolved views set by the last `OMSetRenderTargets` (with
  `RTsSingleHandleToDescriptorRange` semantics resolved), or none.
* `pending_clear[i]`, `pending_clear_ds`: a full-resource clear since the target was bound (or last
  used). Value, and for depth/stencil which planes.
* `pass_open`, the attachments of the open pass, and per attachment a `used` flag (drawn to).
* `discard_pending[i]`: `DiscardResource` seen while the pass is open or right after it.

### 2.2 Begin and end

A pass **begins lazily at the first draw, mesh dispatch or clear that needs it**, not at
`OMSetRenderTargets` (applications set targets, set state, and change their mind). It **ends** at the
first of:

| Event | Why |
|---|---|
| `OMSetRenderTargets` with a different attachment set (any view differs, or count) | Metal attachments are fixed per encoder. Setting the *same* views again is a no-op |
| `Dispatch`, `CopyBufferRegion`, `CopyTextureRegion`, fills, `ExecuteIndirect` of compute, and every other non-render command | the encoder kind changes (a render encoder cannot dispatch or blit) |
| A resource barrier that transitions an **attachment** away from RENDER_TARGET / DEPTH_WRITE (RT→SRV, RT→COPY_SOURCE, ...) | the pass is complete; its store must happen before the read |
| `ResolveSubresource` of an attachment | folded into the store action (see 2.4) when it follows the pass directly, else after the end |
| `ClearRenderTargetView`/`ClearDepthStencilView` of an attachment **that was drawn to in this pass** | a clear cannot be a load action any more; see 2.5 |
| `Close()` (list end), or `ExecuteBundle` that changes bound targets | list boundary |

Barriers that do **not** end it: transitions of resources that are not attachments of the pass
(textures sampled by the next draws, buffers), UAV barriers, aliasing barriers of other resources.
They go into the existing deferred-barrier machinery and are encoded inside the render encoder with
render stage masks (`barrierAfterEncoderStages:beforeEncoderStages:` accepts vertex, fragment,
object, mesh stages) when something earlier in the pass produced the data (fragment-writes UAV read
by a later draw's vertex stage), else as the encoder-start queue barrier.

### 2.3 Load actions

At `BEGIN_RENDER_PASS`, per attachment:

| Situation | load action |
|---|---|
| `pending_clear` of this attachment, full resource, same mip/slice view | `CLEAR` with the value (clear colour is sent as 4 floats, depth f32, stencil u8); consumes the pending clear |
| the resource was just discarded (`DiscardResource` with no write since) or comes from an aliasing barrier / `UNDEFINED` placed state | `DONT_CARE` |
| everything else (the common case: continue rendering to an existing image) | `LOAD` |

Rule of thumb: **never emit LOAD for contents the application cannot read back**. D3D12 committed
resources are zero initialised; the front does not track that, a never-written target is `LOAD` of
zeros (correct, cost accepted).

### 2.4 Store actions

The store action is only known at the end of the pass, but Metal wants it at begin (it sits in the
descriptor). The front **back-patches**: `BEGIN_RENDER_PASS` is written into the list buffer with
`STORE` for everything and the offset of its store fields is remembered; at the end of the pass the
front overwrites them (the encoder has not been handed to anything yet, the list is still in the
front's buffer). Decision at end:

| Situation | store action |
|---|---|
| default | `STORE` |
| `DiscardResource` follows before any other use (the discard arrives after the pass is closed: the front keeps the patch point alive until the next *non*-barrier, non-discard command, exactly like the deferred barriers of `record.rs`) | `DONT_CARE` |
| `ResolveSubresource` of a multisampled attachment follows | `MULTISAMPLE_RESOLVE` into the single-sample destination (carried in the attachment as `resolve_res`/`resolve_view`), plus `STORE` only if the multisample image is used again later (the front does not know: `DONT_CARE` for the multisample image when the target was created with no `ALLOW_...` reuse is a later optimisation) |
| depth/stencil attachment whose resource is only ever used as DSV in this list and is then discarded or transitioned to `DEPTH_WRITE` for the next frame... | `DONT_CARE` is **not** inferred; only the explicit discard above |

### 2.5 Merging passes (TBDR)

A pass costs a tile store and a later tile load per attachment. The goal is therefore **one render
encoder per run of draws over the same attachments**, whatever the application does in between:

1. Same attachment set: stay open across `SetPipelineState`, root arguments, vertex/index buffers,
   viewport and scissor, blend factor, stencil reference, topology and resource barriers that do not
   touch attachments.
2. `OMSetRenderTargets(A)` after `OMSetRenderTargets(B)` and back to `A` (a pattern of shadow-map or
   G-buffer passes): the second `A` is a **new pass** with `LOAD`; do not try to reopen. Metal 4 has
   `MTL4RenderEncoderOptions` *suspending/resuming*: an encoder may be marked "resuming" to continue
   a suspended one of an earlier command buffer. That is the later tool for passes that span several
   command lists (typical in D3D12: one list per pass group): `BEGIN_RENDER_PASS.flags` has
   `SUSPEND_AT_END` / `RESUME_FROM_PREVIOUS` bits reserved for it, set only when the front can prove
   that no other command touches the attachments between the lists (it can: same queue,
   consecutive `ExecuteCommandLists` entries). Not for the first version.
3. **A clear of something already drawn in the pass.** Cheapest correct answer: the front emits
   `CLEAR_ATTACHMENT_RECTS` (0x0520 below), a back-internal full-attachment quad that writes the
   value with the right write mask. It keeps tile memory contents of every other attachment. Ending
   and restarting the pass with `CLEAR` is only chosen when *all* attachments are cleared.
   Partial-rectangle clears (`ClearRenderTargetView` with rects, `ClearDepthStencilView` with rects)
   always take this path.
4. Compute between draws of the same attachments splits the pass: Metal 4 has no compute inside a
   render encoder. Reordering (sink the dispatch below later draws) needs a dependency analysis the
   front's state tracker could provide but is *not* proposed now. Measure first (the scene's
   dispatch/draw interleave count; the game is compute-heavy, so this is where the cost may be).
5. `END_RENDER_PASS` is explicit in the stream (not implied), so the back never has to guess where an
   encoder ends and so a pass with no draws is representable (clear-only pass = `BEGIN`, `END`).

## 3. Opcodes

All fixed size. Field types as in proto (`u32`, `f32`, `ResId`, `PsoId`); `_pad` omitted from the
tables. Unless noted, flags are 0 now.

### 3.1 Passes and bindings, `0x06xx`

| Opcode | Name | Payload | Notes |
|---|---|---|---|
| 0x0600 | `BEGIN_RENDER_PASS` | `rt_count u32`, `flags u32`, `rt[8]: RenderAttachment`, `ds: DepthAttachment` | see below; the store fields are patched by the front at pass end |
| 0x0601 | `END_RENDER_PASS` | none | |
| 0x0602 | `SET_RENDER_ROOT_ARGUMENTS` | *none, reuse* `SET_ROOT_*` | the existing root commands apply to whichever encoder is open (see 3.3) |

`RenderAttachment` (one per target slot, `res == 0` = unused slot):

```
res: ResId                  // texture (m3-tex object)
format: u32                 // the view's format id (DXGI number): may differ from the resource's (sRGB views)
dimension: u8, mip: u8, _: u16
first_slice: u32, slice_count: u32   // array slices / 3D depth slices of the view (layered rendering: 1 for now)
load: u8, store: u8, _: u16          // LOAD / CLEAR / DONT_CARE ; STORE / DONT_CARE / MULTISAMPLE_RESOLVE
clear: [f32; 4]
resolve_res: ResId, resolve_mip: u8, resolve_slice: u32     // for MULTISAMPLE_RESOLVE
```

`DepthAttachment` is the same with `clear_depth f32`, `clear_stencil u8`, a `planes` mask (depth,
stencil) and **separate** load/store actions for the stencil plane. `rt_count` is the highest used
slot + 1 (matches the pipeline's `rt_count`: the back checks they agree at the first draw, as it
checks the root signature, `INVALID_ARG` otherwise).

Sample count, render target array length and size are properties of the textures; the back reads
them from the m3-tex objects, so the pass has no size fields. All attachments must agree
(`INVALID_ARG`).

### 3.2 Render state, `0x050x`

| Opcode | Name | Payload | Notes |
|---|---|---|---|
| 0x0501 | `SET_VIEWPORTS` | `count u32`, `vp[16]: {x, y, w, h, min_depth, max_depth: f32}` | D3D12 `RSSetViewports`; count 0..16 (Metal's maximum). Sent in full, the back applies up to `count`. D3D12 and Metal agree on origin (top-left), clip-space Y up and depth 0..1: no flip |
| 0x0502 | `SET_SCISSORS` | `count u32`, `rect[16]: {left, top, right, bottom: i32}` | D3D12 rectangles are edges, Metal wants origin + size; converted in the back and clamped to the attachment size (Metal rejects a rectangle outside it, D3D12 clamps) |
| 0x0503 | `SET_BLEND_FACTOR` | `f32[4]` | `OMSetBlendFactor`; consumed by the `CONSTANT_*` blend factors the pipeline was made with |
| 0x0504 | `SET_STENCIL_REF` | `u32` | `OMSetStencilRef`; only the low 8 bits matter |
| 0x0505 | `SET_PRIMITIVE_TOPOLOGY` | `topology u32` | list/strip variants of point, line, triangle: `POINT_LIST`, `LINE_LIST`, `LINE_STRIP`, `TRIANGLE_LIST`, `TRIANGLE_STRIP` (proto's own values). Adjacency, patch and (see 6) rect lists are *not* in the set. The back checks the class against the pipeline's `topology` (a draw with a mismatch is `INVALID_ARG`) |
| 0x0506 | `SET_INDEX_BUFFER` | `va u64`, `size u32`, `format u32` (`U16`, `U32`) | the front resolves the view's GPU address; the back keeps `va`, size and format and turns a draw's start index into a byte offset |
| 0x0507 | `SET_VERTEX_BUFFERS` | `start_slot u32`, `count u32`, `vb[16]: {va u64, size u32, stride u32}` | up to 16 (D3D12 has 16 slots; the pipeline's layout uses `render::MAX_VERTEX_SLOTS` = 25); the back writes `setAddress:attributeStride:` into the argument table at `VERTEX_BUFFER_BASE + slot` (6 + slot). Strides are dynamic because `render::vertex_descriptor` makes every layout `MTLBufferLayoutStrideDynamic` |

Not carried: depth bounds (`OMSetDepthBounds`, Metal has none: the pipeline flag travels so the
back can log, the command is not sent), variable-rate shading, `SetViewInstanceMask`,
`SetSamplePositions`, predication (`SetPredication`: separate item), `IASetPrimitiveTopology` for
adjacency/patch. The front returns `E_NOTIMPL` with the existing `note_notimpl` log.

Applied by the back when binding state to the encoder: cull mode, fill mode, winding, depth clip,
depth bias (`slope scale`, `clamp`) from `GraphicsPipeline::raster()` and the `MTLDepthStencilState`
from `GraphicsPipeline::depth_stencil_state()` at **pipeline bind**, not by commands (they are baked
into the proto PSO). Redundant binds are skipped by `GraphicsPipeline::identity()`.

D3D12's `DepthBias` is an integer in units of the smallest representable depth difference for the
format; Metal's `setDepthBias` takes a float scale in *the same sense* ("the smallest resolvable
difference", scaled). They agree for UNORM depth; for `D32_FLOAT` D3D12 bias is relative to the
exponent range of the primitive, Metal's likewise (both follow the hardware formula). Verify with a
test against D3DMetal's output on a biased shadow map before trusting it (open item 4).

### 3.3 Draws, `0x051x`

| Opcode | Name | Payload | Notes |
|---|---|---|---|
| 0x0510 | `DRAW` | `vertex_count`, `instance_count`, `start_vertex`, `start_instance` (u32) | `DrawInstanced` |
| 0x0511 | `DRAW_INDEXED` | `index_count`, `instance_count`, `start_index` (u32), `base_vertex i32`, `start_instance u32` | `DrawIndexedInstanced`; needs `SET_INDEX_BUFFER` first |
| 0x0512 | `DISPATCH_MESH` | `x`, `y`, `z` (u32) | `DispatchMesh`: object-threadgroup counts when the pipeline has an amplification stage, mesh-threadgroup counts otherwise (the D3D12 meaning of the three numbers) |
| 0x0520 | `CLEAR_ATTACHMENT_RECTS` | `target u32` (0..7, or 8 = depth-stencil), `flags u32` (planes), `rect_count u32`, `value[4]` or depth/stencil, `rect[8]` | mid-pass clear (2.5): drawn by a back-internal pipeline made once per colour format / depth format |

Before each draw the back
1. checks a pass is open (if not, `INVALID_ARG`; the front guarantees it by emitting `BEGIN` lazily),
2. writes the TLAB snapshot if root arguments are dirty (ring allocation, as `DISPATCH` does today),
3. for pipelines whose vertex reflection says `needs_draw_params` or for any mesh pipeline writes the
   **draw argument** (`IRRuntimeDrawArgument` / `IRRuntimeDrawIndexedArgument`, 16 or 20 bytes) into
   the ring and binds it at bind point 4 (`kIRArgumentBufferDrawArgumentsBindPoint`), and the **draw
   info** (`IRRuntimeDrawInfo`: `indexType` = non-indexed / 16 / 32, plus for mesh pipelines the
   `primitiveTopology` and the threadgroup strides MSC documents) at bind point 5
   (`kIRArgumentBufferUniformsBindPoint`). The converter's own helpers
   (`IRRuntimeDrawPrimitives`...) take a classic render encoder object and are not used; the back
   writes these structs itself from the header's definitions (layout asserts in `msc/ffi.rs` style),
4. issues `drawPrimitives` / `drawIndexedPrimitives` / `drawMeshThreadgroups` with the
   `MTLPrimitiveType` from the topology (strips and lists), `baseInstance` and `baseVertex` natively,
5. adds the draw to stats (`ListStats`: draws, passes, mesh draws).

TLAB and heaps in the render encoder: `SET_DESCRIPTOR_HEAPS`, `SET_ROOT_SIGNATURE`, `SET_ROOT_*`
(`cmd.rs`) keep their meaning; they update the same per-list `bound` shadow; the back does
`setArgumentTable(table, atStages: vertex | fragment | object | mesh)` at pass begin (and
`SET_PIPELINE` on a graphics pipeline checks that the stages' layout equals the root signature's,
already verified at creation: `exec/gfx.rs`). The argument table must be created with
`supportAttributeStrides = true` and 31 buffer binds (today `descriptors::ArgumentTable` is made
for compute: one field to add). The heaps' residency is the existing residency set of the command
buffer; render targets, vertex/index buffers are texture/buffer objects already in it.
Metal snapshots the table at each draw, so, as for dispatch, an address set after a draw affects only
later draws.

### 3.4 What the interpreter needs (phase 2 back)

* `record.rs`: an `EncoderKind::Render`; `begin_render_pass(&RenderPassDesc)` builds the
  `MTL4RenderPassDescriptor` (colour attachments from textures: m3-tex), ends the compute
  encoder, starts a render one with the consumer-form queue barrier of every encoder start; the
  shadow-state apply; draw methods. The barrier tracker gets render stage masks.
* `exec/stream.rs`: the new commands; `bound.pso` already can be graphics (`PsoBody::Graphics`).
* `stages.rs`: `VERTEX`, `FRAGMENT`, `OBJECT`, `MESH` bits exist as barrier stage masks (check).
* One built-in pipeline set for `CLEAR_ATTACHMENT_RECTS`, and later for `ResolveSubresource` of
  depth (Metal has `MultisampleResolve` store for colour only; depth min/max/sample0 resolve needs a
  shader or a store option).

## 4. Order of work (suggested, smallest visible output first)

1. proto + front + mock + goldens for 3.1/3.2/3.3 (no GPU), `DrawInstanced` into the mock with the
   pass derivation unit-tested over recorded D3D12 call sequences (clear, set RT, draw, barrier,
   draw, discard) in the front: the pass derivation is pure and is where bugs will be.
2. back: render pass + `DRAW` into a texture, readback by `CopyTextureRegion` (needs m3-tex), a
   triangle test with pipelines from phase 1's HLSL; then `DRAW_INDEXED`, vertex buffers,
   `DISPATCH_MESH` and the draw-arguments binding.
3. present (0x07xx), which is the swapchain path and not this document.

## 5. Why not the alternatives

* **The back derives passes from raw D3D12-shaped commands** (`OM_SET_RENDER_TARGETS`, `CLEAR_RTV`
  as commands, back infers). It needs the front's resource-state knowledge (discard, barriers)
  duplicated or sent as extra commands, and any wrong guess is a visible image bug; the front has
  the facts. Rejected.
* **Metal render-pass descriptors built eagerly at `OMSetRenderTargets`.** Costs an empty pass for
  every set-then-change-mind (common) and cannot see a clear that follows. Lazy begin is the
  standard approach for D3D12-on-tile translation layers.
* **One MTLDepthStencilState and rasterizer state as proto objects with their own create calls**
  (the question of phase 1). Decided against: the D3D12 object model has them inside the PSO, the
  game never changes them apart from the PSO, so the back creates and caches the
  `MTLDepthStencilState` per pipeline (`GraphicsPipeline::depth_stencil_state`). A shared cache
  keyed by the state could be added in the back if pipelines with identical depth state become
  numerous (Metal deduplicates the objects anyway); no wire change needed.

## 6. Open items

1. **Rectangle lists.** The upstream host creates a geometry shader for rectangle-list topologies
   (`pipeline/host.rs`); phase 1's `to_proto` refuses geometry shaders (`E_NOTIMPL`). Metal has none.
   MSC can emulate a geometry stage through mesh (the runtime exports passthrough geometry shader
   names, `IRCompilerSetInputTopology`, `kIRTrianglePassthroughGeometryShader`), but the project has
   not tried it. Needs the game's actual use: if rectangle lists are only full-screen/UI quads, the
   front can rewrite the draw as a triangle strip with a back-supplied index pattern
   (`DRAW_RECTS`) and a vertex shader prologue; otherwise MSC's emulation. First thing to check with
   the supervisor (it decides whether those 3 `DrawInstanced` sites work at all).
2. **Mixed vertex attribute types.** The survey's two failures: a shader whose vertex inputs have
   different component types (float and integer) needs per-attribute formats from the *input
   layout* (the application gives them), fine for real layouts; no work here, only noting that
   the survey fed one format to all.
3. **Mesh limits.** 11 of 316 cached mesh shaders exceed Metal's total mesh output size
   (`Total mesh size (34816) exceeds the maximum mesh size allowed (32768)`); phase 1 reports it as
   `UNSUPPORTED` with the numbers. A real answer is shader-side (smaller meshlets) or a fallback path
   (m1-brief's "lower the output counts and dispatch more groups" is not valid for fixed
   `SetMeshOutputCounts`): decision for the supervisor.
4. **Depth bias equivalence** and **strip-cut** (Metal always restarts strips on the maximum index
   when the draw is indexed; D3D12 does so only if the cut value is enabled): test, then decide.
5. **ExecuteIndirect** for draws (0x0900) and **timestamp queries** are M2 and not drafted here.
6. **Pass spanning command lists** (2.5 item 2): measure first, then use suspend/resume.
7. `MultisampleResolve` of depth and stencil resolves (`ResolveSubresource` on depth): a shader.
