# Translation map (v1, 2026-11-27)

Two chains, every hop with its input, its output and where the rule lives (new base, branch `metal-livetr` 0093515c).
Every unit of every hop is a row in `docs/metal/translation-matrix.csv.txt` (ids below); `tools/metal/matrix.sh` merges it
with the Mac lead's `docs/mac/translation-matrix-mac.csv` and writes the lists the work is taken from:

* `docs/metal/translation-unproven.md` — used=yes and not proven, by frequency. **The only source of bug-fix work.**
* `docs/metal/translation-stubbed.md` — rows with disposition=stubbed (what is skipped or faked), with the way out.
* `docs/metal/translation-priority.md` — the same rows (used, not proven) sorted by priority = log10(freq+1) × max(symptom) ÷ cost; columns `symptom` and `cost`, format in `docs/metal/symptom-format.md`.

Working method: make each node right, in order; when every node is right, the picture is right.

## Words used in the matrix

| column | values |
|---|---|
| owner | `metal` this line (front/proto/back), `shared` shared translator or victory-gpu (annotated only, not changed here), `mac` loader patches, Rosetta, HLE memory, Wine (Mac lead fills; skeleton rows only), `ref-only` D3DMetal's own behaviour (its defects, no-op pipelines, out-of-range SetRenderVertexBuffers): D3DMetal is a reference, not a target (user, 2026-11-28 00:06); `disposition` is `n/a`, the row is not a gap and `tools/metal/matrix.sh` leaves it out of coverage and of both lists; `reference` may stay `d3dmetal` |
| used | `yes` seen in the title (freq says how often, `freq_source` where from), `no` looked for and absent, `unknown` not measured |
| status | `proven` a test with an independent reference (spec, AMD hardware, D3DMetal) passes on our path; `unproven` used but no such test, or only self-consistency tests; `gap` not implemented, stubbed, unsupported |
| disposition | how the AMD hardware/driver part of the node is handled on the Mac path: `mapped` (same thing, other name), `translated` (rule-based conversion), `emulated` (done in software, differs in cost or edge cases), `stubbed` (accepted and ignored, or skipped), `unsupported` (refused) |
| reference | `hw` the test runs the translated program on a real GPU and compares with expected values from the ISA rules (the GPU is the host's, not an Xbox: no measured Xbox value exists), `spec` the expected value is computed from the D3D12/ISA rules, `d3dmetal` the same test run against D3DMetal, `none` |
| xbox_hw / deviation / restore_plan | what AMD hardware or driver does implicitly in this node / what is different on the Mac and what may show / highest-fidelity Metal 4 route with cost and feasibility (可行 / 待验证 / 不可行) and dated sources. Cost is recorded, never a reason to choose |
| symptom | what the title shows when this node is wrong: `geomN colorN flickerN slowN crashN`, each 0–3 (geom: missing geometry; color: wrong, white or dark color; flicker: flicker, an unstable picture; slow: slow frames; crash: crash). Then optional tags: `估` the scores are an estimate, `freq=floor` a measured reproducible symptom whose freq is words only, `closed` fixed today and not ranked, `summary` a row that stands for rows of its own |
| cost | person-hours to close the node (from `research/map-plan/plan.tsv`, 1 d = 8 h, ranges take the middle); `估` after the number when the plan has none (4 h when nobody is on it) |

Frequencies come from: `callstats:20261127-221938-map` (calls at seam A in one game run, 370 s, the title still early),
`census:2026-11-27` (programs of the package, `research/census/census.json`), run logs (r6, the map run). Frequencies of
different sources are not comparable with each other.

## Private SDK (GDKX)

GDKX may be used freely while developing (semantics, structure layouts, oracle numbers, the constants and structures we
need). Do not commit whole headers, documentation or the `private/` directory. See `metal/README.md`.

## Chain 1 — shader

`GFX10 ISA (RDNA2 binary in the title) → victory-shader decode/IR/lower → DXIL → Metal Shader Converter 4.0.3 → AIR/metallib → MTL4Compiler pipeline`

| hop | input | output | rule lives in | units (matrix ids) |
|---|---|---|---|---|
| 1 decode | console shader container (XDAT / XBCH / FSSM / SGD5 packed), GFX10 machine code | decoded instruction list with fields | `crates/victory-shader/src/native.rs` (container, program headers, root tables), `gfx10/mod.rs`, `gfx10/opcodes.rs` | S-I-001 |
| 2 IR + structure | instructions | SSA-like program, basic blocks, structured control flow, lane masks, IO description | `ir.rs::build` (hints dropped at 1296-1311, wave64 mask pairs), `structure.rs` (divergent if/loop, irreducible split), `io.rs` (interpolation, exports, mesh IO), `spill.rs` | S-I-002..006, S-I-017, S-G-001..009 |
| 3 lower | IR | DXIL values/instructions, resource usage, host root signature | `emit/lower.rs` (per instruction class), `emit/wave.rs` + `emit/mod.rs:517-830` (wave64 on 32 lanes), `resource.rs` (V#/T#/S# → heap views, typed/raw/structured, waterfall), `emit/intersect.rs` (BVH), `emit/rect.rs` (RECT_LIST GS), `emit/sampler.rs` | S-I-007..024, S-G-010, S-G-011 |
| 4 container | DXIL program | DXBC-wrapped DXIL container with PSV, signatures, root signature, hash | `emit/bitcode.rs`, `emit/container.rs`, `emit/validator.rs` | S-D-001 |
| 5 MSC | DXIL container + compile options (`back/src/msc/compile.rs`: validation flags, input topology, GS/tessellation emulation, root signature, stage-in) | metallib + reflection | `metal/back/src/msc/{ffi,compile,dxil,pipeline,root}.rs`, libmetalirconverter 4.0.3 | S-M-001..005 |
| 6 pipeline | metallib functions + pipeline state | `MTL4*PipelineState` (render, mesh, compute), archive | `metal/back/src/pipeline.rs`, `render.rs`, `msc/pipeline.rs`, MTL4Compiler in `back/src/device.rs:137` | S-P-001 |

Minimal units per hop: instruction classes (scalar ALU, scalar memory, control flow, hints, VALU f32/f16/f64/int,
transcendentals, lane/wave ops, LDS, GDS/ordered append, buffer, image, FLAT, export/interp, BVH), operand forms and
modifiers (wave64/32 and lane masks, f16/f64, denorm/NaN/zero rules, omod/clamp/abs/neg, literals), resource kinds in
shaders (typed / raw / structured buffers, BC and uncompressed textures, Xbox tiling reached through title memory,
descriptors selected per lane), stages (VS, PS, CS, MS/AS, GS/ES, none of LS/HS/DS in the package).

AMD implicit behaviours with their own rows: wave64 + lane mask S-I-015/016/017; denorm/NaN S-I-003/S-I-013; Xbox tiling
and BC C-V-003/C-V-005/C-F-002; DCC C-V-006; HiZ/HTile C-V-007; depth bounds C-F-027; 64-bit atomics S-I-020; GDS and
ordered append S-I-019; BVH instructions S-G-010, BVH builder S-G-011; RDNA2 scalar `s_*` S-I-002..006.

## Chain 2 — command

`D3D12.X in the title → victory-gpu (console objects, shader cache, pipelines, descriptors, memory) → host D3D12 (seam A) → front → proto (seam B) → back → Metal 4`

| hop | input | output | rule lives in | units |
|---|---|---|---|---|
| 1 title → victory-gpu | console D3D12.X calls and console memory (descriptors, pipeline streams, tiled textures) | PC D3D12 calls on the host device | `crates/victory-gpu/src/`: `device.rs` (methods, unknown slots 0x1c8/0x218 at :349), `pipeline.rs`, `pipeline/{make,host,library}.rs`, `shader_cache.rs`, `descriptor/host.rs`, `texture.rs` (detile), `resource.rs`, `tiles.rs`, `indirect.rs` (argument clamp), `queue.rs`, `list.rs` | C-V-001..017 |
| 2 seam A | the host D3D12 / DXGI interface | — (the front implements it) | `metal/front/d3d12/src/lib.rs`, `metal/front/dxgi/src/lib.rs`, generated `front/src/shell/stubs.rs` (`tools/metal/gen-front-stubs.sh`), `VICTORY_METAL_CALLSTATS` counters in `front/src/callstats.rs` | counted per method |
| 3 front core | D3D12 objects, descriptors, root signatures, PSO descs, barriers, copies | proto commands (own objects, recorded lists) | `front/src/{device,resource,view,descriptor,rootsig,convert,pso,render,state,footprint,format,caps,indirect,timestamps,swapchain,library}.rs`, `front/src/shell/{device,list,ext,swapchain}.rs` | C-F-001..073 |
| 4 proto (seam B) | proto v5 command blocks | encoded blocks over the bridge | `metal/proto/src/*`, `metal/bridge/src/{abi,pe,unix}.rs` (`OP_PROTO_SUBMIT`, `OP_PROTO_CALL`) | C-P-001, C-P-002 |
| 5 back → Metal 4 | proto commands | MTL4 command buffers/allocators/queue, argument tables, residency sets, heaps, textures | `back/src/exec/{mod,stream,gfx,render_cmds,indirect,sync,present}.rs`, `draw.rs`, `copy.rs`, `barrier.rs`, `stages.rs`, `memory.rs`, `heap.rs`, `texture.rs`, `typed.rs`, `descriptors.rs`, `residency.rs`, `queue.rs` | C-B-001..004, C-B-010..013 |
| 6 present | back buffer of the swap chain | `CAMetalLayer` drawable | `front/src/swapchain.rs`, `back/src/present.rs`, `exec/present.rs` | C-F-070, C-F-071 |
| 7 resolution | the title's output size, render scale and upscaler | patched title code (1280x720 on a Mac) | `apps/victory/src/mac.rs:68-75` → `crates/victory-loader/src/mac/resolution.rs` | C-R-001..008 |
| 8 Mac host layer | Windows process under Wine on Rosetta | — | `docs/mac/PATCHES.md` | C-X-001..016 (skeleton, Mac lead) |

Minimal units per hop: resource types and formats (buffers; 1D/2D/3D; BC1..7; Xbox tiling; typed/raw/structured views),
descriptors and root signatures (table, root constant, root view, static sampler), resource state and barriers
(transition pairs, UAV, aliasing), pipeline state (blend, depth/stencil, rasterizer, GS, mesh limits, conservative
raster, barycentrics, depth bounds), indirect execution, queries and timestamps, present, copy and clear operations,
fences/residency.

## Where the chains meet

* A pipeline needs both chains: a shader that the shader chain cannot make (e.g. `attributeAtVertex`, S-G-002) makes the
  command-chain pipeline fail (C-V-010, C-B-011) and its draws vanish.
* Console memory read by the GPU is reached by title memory directory/page cache (S-I-024) and by CPU detiling (C-V-003).

## How to read the first numbers

See the report of the commit that added this file, and run `tools/metal/matrix.sh`.
