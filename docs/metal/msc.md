# Metal Shader Converter in the backend (M1, shader part, phase 1)

What was done, what was measured and what it means for `victory-shader`.
Checked 2026-11-27 on an Apple M5 Max, macOS 27.2.1, Rust 1.98.1. Code:
`metal/back/src/msc/`; tests: `metal/back/tests/{msc_gpu,msc_features,msc_survey}.rs`.
Raw per-shader results are outside the repository:
`~/Documents/GTA6Project/research/probe-results/m1-msc-*.txt`.

## Installation (read 2026-11-27)

| What | Where |
|---|---|
| Tool | `/usr/local/bin/metal-shaderconverter`, `metal-irconverter version: 4.0.3`, default triple `air64-apple-macosx15.0.0`. `/usr/local/bin` is **not on `PATH`**: use the absolute path. |
| Library | `/usr/local/lib/libmetalirconverter.dylib`, 60 MB, **universal `x86_64 arm64`** (`lipo -archs`), install name `@rpath/libmetalirconverter.dylib`, about 116 exported `IR*` C functions. |
| Headers | `/usr/local/include/metal_irconverter/` (compiler, `ir_*.h`, `ir_version.h`: 4.0.3), `/usr/local/include/metal_irconverter_runtime/metal_irconverter_runtime.h`, `/usr/local/include/metal_irconverter_ext/Metal_HLSL.inc` |
| Documentation | `/opt/metal-shaderconverter/docs/UserManual.md` |
| Version | 4.0.3 (`IR_VERSION_MAJOR.MINOR.PATCH`). Called "4.0 beta 3" in saxaboom's PR #40; the installed tool prints 4.0.3. |

`IRCompilerGetBinaryVersion` is a C++-mangled export (`__Z26IRCompilerGetBinaryVersionPh`), not
callable from C, so the library's version is read from the header macros only
(`msc::HEADER_VERSION`), not from the loaded binary.

## Header review: the installed header against saxaboom `efd8085`

`descriptors.rs` was written from the copy vendored in `Traverse-Research/saxaboom` at
`efd8085` (`bindings_generator/vendor/`). Compared file by file on 2026-11-27:

* `metal_irconverter.h` and every `ir_*.h` in the main header directory are **byte-identical**.
* `metal_irconverter_runtime.h` differs only in layout, not in a value: the installed one
  adds `extern const` declarations of the `kIR*` constants and moves the
  `IR_PRIVATE_IMPLEMENTATION` block; `ir_raytracing.h` additionally declares
  `kIRRayDispatchIndirectionKernelName` and `kIRRayDispatchArgumentsBindPoint` as `extern`;
  `ir_tessellator_tables.h` has its constants moved/`extern`. No semantic difference.
* Checked in the installed header and equal to `descriptors.rs`: `IRDescriptorTableEntry` is
  three `u64` (`gpuVA`, `textureViewID`, `metadata`); buffer metadata is the size in the low 32
  bits, the texture-view offset in 8 bits at bit 32, the typed flag at bit 63; a texture entry has
  `gpuVA = 0`, `textureViewID = resource id`, `metadata = minLODClamp bits | metadata << 32`; a
  sampler entry has `gpuVA = resource id`, `textureViewID = 0`, `metadata = lodBias bits`; bind
  points `DescriptorHeap 0`, `SamplerHeap 1`, `ArgumentBuffer 2`, `HullDomain 3`, `DrawArguments 4`,
  `Uniforms 5`, `VertexBuffer 6`, `StageInAttributeStart 11`.
* Measured again 2026-11-27 (4.0.3): the texture-view offset field is not applied to typed buffer
  reads at all (see `m1-exec.md`, typed views: misaligned SRVs use a mirror).
* The manual's prose still says the texture-view offset is "2 bytes"; the header uses an 8-bit
  mask. The code follows the header (unchanged).
* The `kIR*` constants are header-only (`IR_PRIVATE_IMPLEMENTATION`), not exported by the
  dylib: a Rust binding has to restate them (`descriptors::bind_point` does, with tests).

Result: **no correction to `descriptors.rs`**; only its doc comment ("header not installed,
to be re-checked") was updated.

`IR_RUNTIME_METAL4`, the switch the task asks about, is a switch of the *runtime companion
header*: it enables `IRRuntime4Draw*` helpers (geometry/tessellation emulation draws through an
`MTL4ArgumentTable`; the scratch buffer is bound at `kIRArgumentBufferDrawArgumentsBindPoint`
and `kIRArgumentBufferUniformsBindPoint`) and the `mtl4*` typedefs. It is not a converter
option and does not change compute: the converter output is the same, and the TLAB address
goes into the argument table at bind point 2 as `descriptors` already does.

## FFI: hand-written `dlopen` bindings, no `saxaboom`

Checked 2026-11-27:

* crates.io `saxaboom`: latest `0.2.0+irconverter-2.0` (2025-05-15): the 2.0 headers, which do
  not have 4.0 (mesh/amplification fixes, `EnsureTGMemoryCoherency`, Metal 4 helpers).
* git `main`: `efd8085` (2026-10-28, PR #40, `0.2.0+irconverter-4.0.3`). It would be a git
  dependency pinned by revision.

Chosen: **own bindings of the 44 functions used, loaded with `dlopen`**
(`msc/ffi.rs`), no new dependency (`libc` is already one; `Cargo.lock` is unchanged). Reasons:

1. saxaboom's wrapper panics where we need a value: `IRError::code()` has no arm for
   `NullHullShaderInputOutputMismatch`, `InvalidRaytracingUserAttributeSize`,
   `IncorrectHitgroupType` and `FP64Usage` and ends in `panic!("Invalid error code")`; a
   conversion that fails with one of those must be an error the interpreter can report, and
   `IRCompilerCreate` / `IRMetalLibBinaryCreate` failures `expect`.
2. A git dependency needs the network and a revision pin in `Cargo.lock`; the crates.io
   release is the wrong MSC.
3. We need about 44 functions and 14 `repr(C)` structs; sizes and offsets were printed with
   `clang` from the installed header and are `const` assertions in `ffi.rs`. Enum values were
   printed the same way.
4. Thread rules are ours to state (below).

Not a separate sys crate: MSC types appear only inside `victory-metal-back` (layering rule 3
holds; `tools/metal/check-layers.sh` is unchanged and green). The module's public API is plain
Rust (`RootSignatureDesc`, `CompileOptions`, `Converted`, `Reflection`), so nothing MSC-shaped
leaves the crate. If a second crate ever needs the bindings, `ffi.rs` moves out whole.

### Loading, both architectures, errors

`Msc::get()` loads on first use. Order: `VICTORY_MSC_DYLIB` (only that, when set), then
`/usr/local/lib/libmetalirconverter.dylib`, then the bare name (dyld search path). The
dylib's install name is `@rpath/...`, which matters only for link-time linking; `dlopen` by path
needs no rpath and no `build.rs`. The universal dylib is used by the arm64 process and by the
x86_64 one under Rosetta (the Wine unixlib's architecture); both pass the GPU tests (below).
When it cannot be loaded, or lacks a function, the error is
`MscError::Library { tried, detail }` whose text lists every path tried, the `dlopen` message
and what to install, and the rest of the backend works without MSC. A failure is not cached.

Thread rules: the header says an `IRCompiler` is not reentrant. Each `Msc::compile` call makes
its own compiler, so calls from any thread are independent (test: 8 threads x 25 conversions
return identical bytes). Per-thread reuse would save the compiler creation only; not measured.

## What the pipeline entry does

`Device::compute_pipeline_from_dxil(dxil, Option<&RootSignatureDesc>, &CompileOptions)
-> Result<ShaderPipeline, MscError>`:

1. `msc::dxil::inspect` reads the container (shader kind, SM, `SFI0` feature bits).
2. The root signature (own Rust types in `msc/root.rs`: constants, root CBV/SRV/UAV, tables with
   ranges, static samplers, flags) becomes an `IRVersionedRootSignatureDescriptor` (1.1).
   Flags map `HEAP_INDEXED -> 0x400`, `SAMPLER_HEAP_INDEXED -> 0x800`, `ALLOW_INPUT_LAYOUT -> 1`.
3. `IRCompilerAllocCompileAndLink` -> `IRObjectGetMetalLibBinary` -> bytes.
4. The existing `compute_pipeline_from_metallib(bytes, entry)` (entry from the reflection;
   `main` for DXC's default).
5. Reflection comes back with the pipeline: `threadgroup_size` (`IRShaderReflectionCopyComputeInfo`;
   mesh/amplification thread counts and payload size likewise), `layout`, `uses_resource_heap`,
   `uses_sampler_heap`, function constants.

Without a root signature MSC lays every resource out inline as a 24-byte descriptor-table
entry (a different TLAB than any root signature gives); with one it is the explicit layout below.
`CompileOptions` has the entry name, the `IRCompatibilityFlags`, the minimum GPU family
and "no validation".

### How the reflection gives the TLAB

`Reflection::layout` is `TopLevelLayout { entries, size, static_sampler_table }`: one
`ArgLocation { kind, space, slot, offset, size }` per root parameter, in parameter order, from
`IRRootSignatureGetResourceLocations` (so MSC's padding is included: 3 root constants take 12
bytes and a following root UAV starts at 16; pointers are 8-aligned, constants 4-aligned;
a root CBV/SRV/UAV and a table are one 8-byte GPU address; root constants are inline). `size` is
the end of the last entry rounded to 8: allocate that many bytes from the constant ring, write the
constants at `offset`, the 64-bit addresses at `offset`, bind with `set_root_arguments`.
Verified on the GPU with constants + root UAV, a table + root UAV + static sampler.

Two facts that are not in the header:

* **Static samplers are not baked into the shader.** MSC adds one more TLAB entry after the root
  parameters, a pointer to a **table of sampler descriptors** (24 bytes each,
  `IrDescriptorTableEntry::sampler`) that the host fills (`static_sampler_table` is its index in
  `entries`). The order of the samplers in that table is assumed to be the order of
  `static_samplers`. The static-sampler GPU test (`back/tests/exec_dxil.rs`) runs and passes with one
  sampler; an order with two or more different samplers is **not verified**. Min/max reduction
  filters are read as the standard ones (`Device::create_sampler` has no reduction mode).
* **Heap use** is not in `IRShaderReflection`: its `ResourceUsages` word followed the kinds of
  resource a shader declares (observed 4, 6, 12 for UAV, SRV + UAV, sampler + SRV + UAV) and had
  nothing for heap indexing. `uses_resource_heap` /
  `uses_sampler_heap` are read from the DXIL `SFI0` feature mask (bit 25
  `RESOURCE_DESCRIPTOR_HEAP_INDEXING`, bit 26 `SAMPLER_DESCRIPTOR_HEAP_INDEXING`), which DXC sets;
  both verified (heap shader: `sfi=0x2000000`; sampler-heap shader reports the sampler heap).
  Heap indexing needs the root signature flags `HEAP_INDEXED` / `SAMPLER_HEAP_INDEXED` and the
  heaps bound at bind points 0 and 1 (`set_heaps`).

## In the interpreter (`CREATE_COMPUTE_PIPELINE`)

`exec/dxil.rs` converts the DXIL the front sends: the proto root signature (`RootSigDesc`) becomes
the MSC description kept with the root signature (`RootSig.msc_desc`); `CREATE_ROOT_SIGNATURE`
asks MSC for the layout (`layout_from_msc`, TLAB offsets from the reflection, static-sampler
table pointer included) and keeps the rule-based layout only as a fallback when MSC is not
installed; `CREATE_COMPUTE_PIPELINE` calls `Device::compute_pipeline_from_dxil`, checks that the
pipeline's reflection agrees with the layout of the root signature, and `DISPATCH` takes the
threads per group from the reflection. The static sampler table is built when the root
signature is made, lives in a descriptor heap in the residency set and is written into the
TLAB at `static_table_offset`. With MSC missing (`VICTORY_MSC_DYLIB` pointing nowhere) the status
is `UNSUPPORTED` and `Executor::last_error` says why; there is no panic.
GPU tests: `back/tests/exec_dxil.rs` (root UAV + constant, `ResourceDescriptorHeap[]`, 8x4
threads from the reflection, static sampler, pipeline tied to its root signature),
`exec_no_msc.rs`. They need DXC (`VICTORY_HELPER_DXC`, `DXC_LIB`), else they print SKIPPED.

## Graphics and mesh pipelines (M3, m3-gfx, phase 1)

`render.rs` + `exec/gfx.rs`. Decisions, with reasons:

* **Stage-in: Metal vertex fetch, not MSC's synthesized stage-in function.** The back builds an
  `MTLVertexDescriptor` from the proto input layout (semantic to the attribute MSC's reflection
  names, `STAGE_IN_ATTRIBUTE_BASE` = 11 added to its index, buffers from base 6) and sets
  `IRCompilerSetStageInGenerationMode(MetalVertexFetch)`. Reasons: the layout is known only at
  pipeline creation, not at conversion, and a descriptor keeps one converted vertex function per
  shader whatever the layout; strides are dynamic (`SET_VERTEX_BUFFERS` gives them), so a layout
  that differs only by stride shares the pipeline; Metal does the format conversion (UNORM, SNORM,
  packed) which a synthesized function would need written per format; the synthesized function
  is a separate library to link and cache.
* **Mesh:** object/mesh threadgroup sizes and the payload size come from the reflection; the
  back checks the Metal limits (payload 16 KB, 256 vertices, 512 primitives, 1024 threads, total
  mesh size) and answers `UNSUPPORTED` naming the number and the limit. Pixel inputs the mesh
  stage does not write are Metal's refusal ("Fragment input(s) ... mismatching mesh shader
  output"), reported as it comes.
* **State:** the MTL4 render pipeline descriptor has no depth, cull, fill, bias or winding. Those
  stay in `GraphicsPipeline` (raster state, `MTLDepthStencilState` made at creation) and are
  applied at pipeline bind in phase 2. Blend, RT formats, sample count/mask and alpha-to-coverage
  are in the descriptor. Depth bounds and geometry/tessellation stages are not carried.
* **Thread safety:** `prepare_graphics` (the MSC step, a fresh `IRCompiler` per call) and
  `Device::graphics_pipeline` (the Metal compile) can run on worker threads; tests build four
  pipelines at once. The two-step split is where an async queue and an `MTL4Archive` lookup
  (keyed by the prepared metallib hash and the state) would go; no cache yet.
* **Reflection merging:** every stage's top-level argument buffer layout must equal the root
  signature's (`Mismatch` otherwise); the pixel shader may write the pipeline's targets (+1 when
  dual-source).

### Survey: pipelines from the game's cache (2026-11-27)

`back/tests/gfx_survey.rs` (ignored; reads the user's cache in place, aggregate numbers only; raw
rows without names or hashes in `research/probe-results/m3-gfx-cache-raw-2026-11-27.txt`, summary
in `m3-gfx-cache-summary-2026-11-27.txt`). 1923 entries, 83 refused by the translator:

| Kind | Shaders | Converted | Pipeline built | Cold build ms (median / p95) |
|---|---|---|---|---|
| Pixel alone (convert + library; Metal has no pixel-only pipeline) | 641 | 641 | library 641 | convert 10.9 / 44 |
| Vertex + default pixel | 166 | 166 | 164 | 6.2 / 14.6 (convert 1.1 / 4) |
| Mesh + default pixel | 316 | 316 | 305 | 28.2 / 138 (convert 5.8 / 20.8) |

The two vertex failures have mixed float/integer inputs (the survey feeds one format; a real
layout does not). The 11 mesh failures are over a Metal limit (e.g. total mesh size 34816 against
32768; the same shaders the upstream log shows): 3.5 % of the mesh shaders need a shader-side or
fallback answer. A second run is 30 to 140 times faster (Metal's own cache): the cold numbers are
what a first start pays, which is the case for the `MTL4Archive` cache.

### DXBC input (`msc/dxbc.rs`)

The game layer's runtime shaders (ambient, composite, indirect helpers) are `cs_5_1` from
`D3DCompile`: DXBC, which MSC refuses (`UnrecognizedDXILHeader`). `Msc::compile` now detects a
container with a `SHDR`/`SHEX` part and no `DXIL` part (`dxil::is_dxbc`) and converts it first
with Microsoft's DXBC to DXIL converter, loaded by `dlopen` as D3DMetal ships it
(`libdxilconv.dylib`, x86_64 only). Interface (DirectXShaderCompiler,
`projects/dxilconv/include/DxbcConverter.h`, `main` branch read 2026-11-27; the library is the
D3DMetal build, version not printed): `DxcCreateInstance(CLSID_DxbcConverter
4900391E-B752-4EDD-A885-6FB76E25ADDB, IID_IDxbcConverter 5F956ED5-78D1-4B15-8247-F7187614A041)`,
vtable `IUnknown` then `Convert(dxbc, size, wide options, &dxil, &dxil_size, &wide diag)`; outputs
are `malloc`ed and freed by the caller. Search: `VICTORY_DXILCONV_DYLIB` (only that, if set), then
`~/Documents/GTA6Project/VIctory-macOS/target/wine-cx/lib/external/D3DMetal.framework/Versions/A/Resources/libdxilconv.dylib`,
then the bare name; the error lists every path with its dlopen message (on arm64: "incompatible
architecture (have 'x86_64')"), so the DXBC path works in the x86_64 process (Rosetta, Wine's
unixlib) and says so clearly in an arm64 one.
Findings: the output is a DXIL 6.0 container without a root signature (the root comes from the
caller, as for every shader of this project: `D3DCompile` under Wine has no embedded root
signatures); SM 5.1 register spaces are carried (test: CBV `b1 space3`, SRV `t2 space1`, UAV
`u1 space1` as root parameters run correctly). `back/tests/dxbc.rs`: DXBC made by
`tools/metal/dxbc-compile.sh` (Wine's d3dcompiler, `VICTORY_DXBC_TOOL`); runs under Rosetta
(`--target x86_64-apple-darwin`), SKIPPED on arm64. Wine's compiler itself cannot build some of the
helper shaders (`indirect-clamp.hlsl`: `Load` on `RWByteAddressBuffer` is not implemented there),
which no converter fixes.

## Tests

| Test | What | Result 2026-11-27 |
|---|---|---|
| `msc_gpu` (12) | loader errors; layout padding with no DXIL needed; root UAV compute; root constants + root UAV; `ResourceDescriptorHeap[]` compute (SM 6.6); sampler heap reflection; table + root UAV + static sampler layout; garbage DXIL rejected; 8-thread conversion | **12/12 on arm64 and 12/12 under Rosetta (`--target x86_64-apple-darwin`)** |
| `msc_features` (1 test, 33 cases) | each construct of the manual's feature table, converted with no root signature; outcome pinned | passes on both |
| `msc_survey` (ignored) | converts every entry of a VIctory shader cache and loads the metallib / pipeline | below |

The GPU tests compare the output buffer with a CPU reference (`t*3+1`; `((t*a+b)^c)+d`;
`src[i]*k+7` through heap indices 17 and 40). The DXIL comes from HLSL through the D3DMetal DXC
front end the repository already builds (`tools/macos/dxc-d3dmetal.sh`; docs/macos.md):
`VICTORY_HELPER_DXC=<front end>` and `DXC_LIB=<D3DMetal libdxcompiler.dylib>`; without both these
tests print `SKIPPED` and pass (no DXIL is stored in the repository: asset policy; no DXC ships
with macOS). Without Rosetta the DXC front end (x86_64 only) cannot run either.

## Acceptance survey

### Real shaders: the VIctory cache

`~/Documents/GTA6Project/victory-data/cache/shaders/<translator>/` (derived from the game by
an earlier run; **read in place, never copied; only the numbers below are committed**).
1,923 translated entries read (a further 83 entries are containers `victory-shader` itself
refused and the cache keeps with a reason: not MSC's concern, 4.1 % of 2,006). Root signature per
shader as the host builds it for that shader alone (`pipeline/host.rs`): user data as root constants
`b0`, root CBV/SRV/UAV in space 1, title-memory tables in space 2, the static samplers, both
heaps indexed (the survey's own cache-entry reader and builder are in `msc_survey.rs`). Release
build, one thread, MSC defaults; two full runs (325 s and 338 s wall) gave the same classes.

| | Cs | Ps | Ms | Vs | all |
|---|---|---|---|---|---|
| translated entries | 800 | 641 | 316 | 166 | 1,923 |
| **converted by MSC** | **800** | **641** | **316** | **166** | **1,923 (100 %)** |
| MSC failures | 0 | 0 | 0 | 0 | **0** |
| metallib loads in Metal (`newLibraryWithData`) | all | all | all | all | 0 failures |
| compute pipelines created (`MTL4Compiler`) | 800 / 800 | - | - | - | 0 failures |
| conversion ms, median / p95 / max | 17.6 / 164 / 519 | 11.2 / 45 / 87 | 5.5 / 20 / 40 | 0.8 / 3.6 / 12 | 7.4 / 150 / 519 |
| metallib bytes, median / p95 | 54 K / 437 K | 41 K / 152 K | 21 K / 68 K | 6.6 K / 15 K | 28 K / 398 K |

Whole set: DXIL in 185.8 MB (median 30 KB, p95 489 KB, max 1.39 MB), metallib out 156.0 MB
(max 953 KB); conversion CPU time 52.7-53.1 s in total (median 7.4-7.6 ms, p95 150 ms, max
510-519 ms); compute pipeline creation median 88-108 ms, p95 1.24-1.28 s per shader (Metal's own
compile of the metallib; it dominates the wall time). All SM 6.6.

What the set uses (DXIL `SFI0`, 1,923 shaders): `ResourceDescriptorHeap[]` 1,873 (all 800 Cs);
`SamplerDescriptorHeap[]` 491; wave operations 1,479; 64-bit integer ops 872; pixel
barycentrics 635 (every Ps); typed UAV loads of extra formats 234; UAVs at every stage 129. None
of: doubles, view id, stencil ref, shading rate, sampler feedback, 64-bit atomics, raytracing.
Compute thread groups: 32x1x1 376, 8x8x1 115, 64x1x1 90, 8x4x1 84, 1x1x1 34, 4x4x4 29, 16x16x1 12.

MSC printed `Warning: Undefined: Shader might rely on forward progress/persistent thread` for
**13** of them (still converted): spin loops. Unknown how Metal runs them; see risks.

Limits of this number: the cache is whatever one earlier run translated, with the translator
options of that run (`host_wave32`, the atomic locks and the other switches in
`victory-gpu/src/shader_cache.rs` are part of the cache key; which values that run had is not
recorded in the folder), so it says nothing about shaders not yet met in the game, and "converted and the pipeline compiles" is not "computes the same". No game
shader was executed.

### Synthetic: the emit tests' DXIL

`cargo test -p victory-shader --lib` on macOS runs 251 tests; most of the DXIL-producing ones
(`synthetic_tests`, `semantics_tests`, `valu_tests`, `memory_tests`, ...) are `cfg(windows)`
because they run the output on a D3D12 device, and `halves_tests` does not compile off Windows.
To get their DXIL without changing the upstream crate, a **scratch copy** in `/tmp` (`git
archive` of this tree, never committed) had `halves_tests` switched off and a hook in
`lower_pruned` that writes the lowered program in the cache's entry format; the tests that do
build on macOS (`census_tests` with every opcode, `mark_tests`, `mesh_index_tests`,
`typed_buffer_tests`, the library tests) produced **455 distinct programs**.

| | Cs | Ps | Ms | all |
|---|---|---|---|---|
| programs | 121 | 310 | 24 | 455 |
| converted | 110 | 301 | 24 | **435 (95.6 %)** |
| `UnsupportedWaveSize` ("Wave size is not supported. Must be 32.") | 11 | 0 | 0 | 11 |
| `CompilationError` ("Interpolation mode for pixel barycentrics is not supported") | 0 | 9 | 0 | 9 |

Conversion median 0.2 ms; every converted compute program (110) made a pipeline. The 20 refusals
are the two constructs in the next section; the real cache has neither. The Windows-only
synthetic tests (which hold the DXIL for descriptor tables, comparison samplers, texture atomics,
gathers) were not covered; running them needs Windows or a Windows test binary under Wine, and
was out of scope.

### Constructs against Apple's list (`msc_features`)

DXC from D3DMetal, converted with no root signature (33 cases; the test file is the full table).
"Accepted" is only "converted": MSC converts several constructs its manual calls unsupported.

| Construct | Manual | MSC 4.0.3 here |
|---|---|---|
| `[WaveSize(32)]`, wave intrinsics, quad ops | yes | accepted |
| `[WaveSize(64)]`, `[WaveSize(16)]` | no, error-detected | **refused `UnsupportedWaveSize`** |
| 64-bit integer ALU | yes | accepted |
| 64-bit atomics: on a bound `RWByteAddressBuffer` / bound texture / texture from the heap | limited | accepted |
| 64-bit atomics on a **buffer taken from `ResourceDescriptorHeap[]`** | limited | **refused `UnsupportedInstruction` (`dx.op.atomicBinOp.i64`)** |
| `SV_ViewID` | no | **refused `UnsupportedInstruction` (`dx.op.viewID.i32`)** |
| sampler feedback (`WriteSamplerFeedback`) | no | **refused `UnsupportedInstruction`** |
| `GetAttributeAtVertex` | - | **refused `UnsupportedInstruction` (`dx.op.attributeAtVertex.f32`)** |
| `SV_Barycentrics` perspective / `noperspective` | - | accepted |
| `SV_Barycentrics` with `centroid` | - | **refused `CompilationError`** |
| `SV_StencilRef` output | no | accepted (converter-side warning-free) |
| `SV_ShadingRate` input | no (VRS) | accepted, MSC warns `Unsupported: shadingrate` |
| rasterizer-ordered view | - | accepted |
| `double` arithmetic | (`FP64Usage` exists) | accepted |
| `Sample(..., clamp)` min-LOD clamp | no, with texture read | accepted |
| pack/unpack intrinsics | no, error-detected | accepted (not checked whether this DXC emits the opcode) |
| mesh, amplification, geometry, ray query in compute, derivatives in compute, `groupshared` 32 KB, dual-source outputs, `SV_Coverage` out, `SV_RenderTargetArrayIndex` from VS | yes / - | accepted |

Accepted-but-unsupported (`SV_StencilRef`, shading rate, ROV, `double`) means MSC ignores or
approximates them; only a GPU comparison can say how. Rejections are errors with a code and a
text (`MscError::Rejected`), never a crash.

## What this means for `victory-shader` and the M1 acceptance

1. **Acceptance of the real shaders is not the problem**: 1,923 / 1,923 convert, and all 800
   compute shaders build pipelines. What remains is *correctness* of the converted code (not
   tested: no game shader ran) and the host mechanics around it.
2. **Wave size.** The backend must tell the translator the host has 32-lane waves
   (`Options::host_wave32`; `victory-gpu` derives it from the host GPU's wave width, and this
   device reports 32): the eleven synthetic compute programs MSC refused ("Wave size ... must be
   32") are the ones translated without it. The cache key already includes it; the `translate`
   caller in the new backend path has to pass it.
3. **Heap 64-bit atomics on buffers** are refused; 64-bit atomics on textures (bound or heap)
   convert. `victory-shader`'s image-atomic locks and the 64-bit typed-resource atomics
   (`SFI0` bit 0x1040_0000 test in `host.rs`) therefore stay on the texture path; a buffer atomic
   through the heap would have to be avoided at translation. No program of the cache uses 64-bit
   atomics at all (`SFI0` bits 22, 23, 28 are clear in all 1,923); a native run may choose
   differently, and nothing here says whether the GPU is correct on them.
4. **`GetAttributeAtVertex` is refused**, so the existing translation (flat inputs made by a
   geometry shader, `#203`) is the only route; it needs MSC's geometry-shader emulation, which
   this phase did not test (a `gs_6_6` converts alone; the emulated draw helpers are the runtime
   header's, and are what `IR_RUNTIME_METAL4` enables).
5. **Centroid-mode barycentrics** are refused; the 635 barycentric pixel shaders of the cache
   all convert, so the shapes the translator emits today are fine, the nine synthetic ones that
   fail are the tests' non-default interpolation.
6. **Static samplers cost a TLAB entry and a sampler table** the interpreter must build and keep
   resident (layout above): the proto interpreter cannot treat them as pipeline state alone.
7. **13 cache shaders carry a forward-progress warning**: if Metal's threadgroup scheduling
   does not give the guarantee, a spin lock (`atomic locks`) can hang the GPU; to be run and
   watched with the real device-lost handling (`queue` status), not just compiled.
8. **Pipeline compile time dominates**: 88-108 ms median, 1.2 s p95 per compute shader,
   against 7 ms for MSC itself. Caching the pipelines and
   compiling off the interpreter's thread are later work.
9. **Descriptor heap and TLAB are the backend's job**: MSC says nothing about residency; every
   buffer behind a heap entry or a root address must be resident (already the rule in `descriptors`).

## Known issues

* The conversion has no timeout and a hostile DXIL could take long; the survey's longest was
  0.5 s. MSC validation can be turned off (`no_validation`) at the price of undefined behaviour
  on invalid input.
* `IRCompilerSetMinimumGPUFamily` / deployment target are plumbed (family) or left unset
  (deployment); the defaults produced working pipelines on this M5 Max. Not tried on an older GPU.
* Graphics, mesh and amplification pipelines exist (section above); draws are phase 2. Rectangle lists
  (geometry shader) are not made: see `proto-render-draft.md`, open items.
* The survey's root signature for graphics stages is per shader; the host shares one root signature
  per pipeline. Conversion of a combined pair is not tested.
* The test DXC is a third-party (D3DMetal) build whose version is not printed by its front end; the
  feature table is pinned to the outcomes it produced.

## Reproduce

```
# MSC: /usr/local/lib/libmetalirconverter.dylib (or VICTORY_MSC_DYLIB)
sh tools/macos/dxc-d3dmetal.sh /tmp/dxc-d3dmetal
export VICTORY_HELPER_DXC=/tmp/dxc-d3dmetal DXC_LIB=<D3DMetal.framework>/Versions/A/Resources/libdxcompiler.dylib
cd metal
cargo test -p victory-metal-back --test msc_gpu --test msc_features
cargo test -p victory-metal-back --target x86_64-apple-darwin --test msc_gpu --test msc_features
VICTORY_MSC_SURVEY_CACHE=<cache>/shaders/<translator> VICTORY_MSC_SURVEY_OUT=<file outside the repo> \
  cargo test --release -p victory-metal-back --test msc_survey -- --ignored --nocapture
```
