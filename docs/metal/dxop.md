# dx.op / DXIL feature proofs (map-driven, 2026-11-28)

Task: close the DXIL -> MSC nodes of the translation map that the game uses and no GPU test covers: run a minimal HLSL
(DXC -> DXIL -> MSC 4.0.3 -> Metal 4 compute/render) on the GPU, read the result back, compare with a CPU reference taken from
the D3D11.3 Functional Spec / DXIL spec. Hardware: **Apple M5 Max** (`supportsFamily` Apple7, 8, 9 and 10 all true), macOS 27.2.1,
MSC 4.0.3, 2026-11-28. Matrix rows: `S-D-002` .. `S-D-017` (docs/metal/translation-matrix.csv.txt); `S-I-020` corrected.

Tests: `metal/back/tests/dxop_{alu,wave,tex,pixel,mesh,atomic,atomic64,int64,typed_uav,rov,dot4_store}.rs`, rig `common_dxop/mod.rs`
(root constants `K`, SRV `I`, UAV `O`; a pixel rig and a mesh rig; every program is checked to declare the dx.op it is meant
to prove). Without `VICTORY_HELPER_DXC` / `DXC_LIB` they print SKIPPED and pass. 44 tests.

## Closed (spec + GPU)

| node | programs | tests |
|---|---|---|
| legacyF16ToF32 (f16tof32) | 14037 | all 65536 halves exact |
| legacyF32ToF16 (f32tof16) | 11651 | see Finding 1 |
| unaryBits.i32 | 13058 | count/reverse/find, edge + random |
| groupId / threadIdInGroup / flattened / threadId | 11378 / 1502 | 3D dispatch |
| wave ops (SFI0 bit 14: 18912) | readLaneFirst 16316, getLaneIndex 2203, activeBit 2124, activeBallot 2141, readLaneAt 1997, activeOp 1081, prefixOp 55, anyTrue 6 | lanes, masks, floats |
| quadReadLaneAt / quadOp | 6155 / 2542 | 1D, 2D group, pixel |
| discard, isHelperLane | 7404, 5025 | Finding 4 |
| sample / sampleBias / calculateLOD / sampleGrad / textureGather / sampleCmpLevelZero | 4068, 3678, 2880, 906, 645 | ps; cs: Finding 3 |
| setMeshOutputCounts / emitIndices / storeVertexOutput | 8765 | mesh draw |
| atomicBinOp.i32 / atomicCompareExchange.i32 | 1638 / 26 | UAV, LDS, ps |
| Int64 ALU (SFI0 bit 15: 6641) | | 4096 pairs per op |
| Typed UAV load additional formats (SFI0 bit 11: 640) | | 26 formats |
| ROV (SFI0 bit 12: 10) | | order-dependent RMW |
| dot4AddPacked.i32 (opcodes 163/164) | 13 cs | signed + unsigned, edge + 2000 random, accumulator wrap |
| textureStore.i32 | 1 cs | 15 integer typed-UAV formats exact; out of range clamps |

Stage split of each node (ps/vs/cs/ms) is in `research/dxop/stage-by-op.tsv` (script `research/dxop/stage_by_op.py`, outside the repo).

## Wave convention

Apple SIMD width is 32 (`threadExecutionWidth` 32; lane = flattened index % 32 in 1D, 2D and 3D groups). The translator targets host
wave 32 (`set_host_max_wave`; console wave64 programs become two host waves, per-wave "local" mode or LDS exchange); `VICTORY_WAVE_WIDE=1`
is off. MSC refuses `[WaveSize(64)]`/`(16)` and accepts 32 (msc_features.rs). The tests run groups of 32/64/128 threads and
confirm the lane math that the translator's `wave_base` idiom relies on (it numbers every thread once).

## Findings

1. **f32tof16 is round-toward-zero in MSC** (spec-conformant, D3D11.3 3.2.2), with overflow saturating (65520 -> 0x7bff, not Inf) and a
   double rounding for half-denormal results (mantissa cut to 10 bits, then nearest-even: 0x33002000 -> 1 where RTZ gives 0). 100 of 5504
   inputs differ from pure RTZ, all |x| < 2^-14, one step 2^-24. Metal's own `half(x)` is nearest-even, so MSC emulates RTZ. The console's
   `v_cvt_f16_f32` rounds to nearest: translated code is 1 ULP low unless the translator's `VICTORY_F16_NEAREST` rounding is on. Not a back defect.
   f16tof32 is exact; NaN payloads are canonicalised (allowed). Followed up in "f32tof16 rounding" below.
2. **64-bit find-first-bit** (`unaryBits.i64`, codes 32-34): MSC converts it to a module Metal cannot materialise ("Failed to materializeAll");
   countbits and reversebits at 64 bits are right. No census program uses `unaryBits.i64` (pinned by a test).
3. **Compute stage derivatives**: MSC drops the SampleBias bias and returns 0 for CalculateLOD ("not supported in stages that support
   compute derivatives"). The pixel stage is correct for Sample, Bias, LOD, Grad, Gather, SampleCmp. In the census these ops are ps only.
4. **Discard**: MSC stops a discarded lane instead of keeping it as a helper; a quad read of a value computed after the discard sees extra
   helper bits (12 of 48 live pixels). Pre-discard values stay readable. Low impact: the translator reads only its own IsHelperLane and
   pre-discard values. Pixel-shader atomics and stores skip helper lanes correctly (36 covered of 64).
5. **2D compute quad ops** follow the 2D spec rule; the lane index there is not the flattened thread index.
6. Wave float ops flush denormals (sum of denormals = 0) and min/max skip a NaN lane (D3D leaves NaN undefined).
7. Heap comparison samplers: the sampler state object must outlive the descriptor that holds its id (the rig keeps it).
8. Mesh interpolation error ~5e-4 vs CPU (tolerance 2e-3).

## atomicBinOp.i64 (S-I-020 said 1 program; the real count is 679)

Census (data/shared/census, `research/map-plan/dxop-per-program.tsv`, 2026-11-28): **679 programs, 677 ps and 2 cs**, all from the
translator's `image_atomic_*` on a 64-bit data mask (one `R32G32_UINT` texel read as `uint64_t`; SFI0 bits 22 and 28 both on in all 679).
ISA of these programs: `image_atomic_or`+`umax`+`umin` in 675 (one `umax` twice in 269) and `umax` alone in 4; which of them is the wide one
cannot be read here (no DXIL disassembler), but each program has one 64-bit atomic.

Support, dated 2026-11-28:
* Metal Shading Language Specification 6.15.4.6 (64-bit atomic modify functions): `min`/`max` of `ulong` only, no returned original;
  Metal Feature Set Tables (developer.apple.com/metal/Metal-Feature-Set-Tables.pdf, note 11, mirrored at
  github.com/kibiyuru/MetalFeatures): "64-bit atomics" from Apple9 (Apple8 partly), on buffers and textures, `RG32Uint` texture needed.
* The premise "Apple10 has no 64-bit atomics" does **not** hold on this machine: `supportsFamily` is true for Apple9 and Apple10 and
  64-bit texture `max`/`min` give exactly the CPU result (test `texture_atomic_max_and_min_on_64_bits`, 16 texels x 4 writers, three
  start values) and in a pixel shader (`pixel_shader_64_bit_atomic_max_skips_helper_lanes`; helper lanes do not write).
* MSC 4.0.3 headers (`/usr/local/include/metal_irconverter/metal_irconverter.h`) say nothing on 64-bit atomics; behaviour was measured:
  min/max without the original on texture 1D/2D/array/3D, typed buffer, structured buffer, byte-address buffer, bound or heap: accepted;
  and/or/xor/add/exchange and any original-value form: refused `UnsupportedInstruction (dx.op.atomicBinOp.i64)`; compare-exchange:
  converted, then Metal refuses ("unlowered function call to air.atomic_compare_exchange_weak_explicit_texture_2d").
  (`msc.md` says heap-buffer 64-bit atomics are refused: with the 4.0.3 here a byte-address heap buffer `InterlockedMax64` builds; the earlier probe `atomic64_heap` used `InterlockedAdd64`, an operation MSC refuses on any resource, so the refusal is about the operation, not the heap.)
* Of the 679, MSC CLI: 616 stop first on attributeAtVertex (owned by the S1 lane; lowered in docs/metal/attribute-at-vertex.md, after which that doc
  measured exactly 1 program still failing on i64), 62 convert, **1 refused**: `PS_Weather_Particle_Prepass_74a7a22c_Wrapped`
  (shaders.rpf@0xbd6ee06, a single `umax`).
* **That program uses the returned original** (disassembled 2026-11-28 with `research/attr-vertex/agx-crash/dxcdis.cpp`, DXC of
  D3DMetal, on the DXIL the translator makes, hash 64974027a11daa59): `%old = atomicBinOp.i64(umax, RWTexture2D<U64>, x, y, (lo | depth << 32))`,
  then `lshr 32`, `trunc`, bitcast to float, `fcmp ult depth, old_high`; the result feeds the branch that ends in `discard` (a weather particle
  depth test: the pixel is discarded when it is behind the stored depth). So the original cannot be dropped, and no `mac:` patch removes it.
  Plan, with a test: read the 64-bit texel, then `InterlockedMax` without the original. A max only grows, so the value read before is never
  above what the atomic would return; "depth < old" can then only be missed under a race (a pixel drawn that would have been discarded),
  never wrongly taken. MSC and Metal take the 64-bit texel read; test `a_64_bit_texel_read_then_max_gives_the_value_before_and_the_max`
  (read = initial value, texel = max; without a race). Not yet in the translator (one program of 26177); left as the S-D-012 plan.

Conclusion: reachable and **native** for 678 programs (min/max); the emulation need is **one** program (the read-then-max above).
The 62 converting programs were converted, not run: running them belongs to the mac-host-harness line (`metal/shader-exec`, branch
`harness-back`; docs/metal/shader-exec.md there), not to this note.

## f32tof16 rounding: the translator's nearest switch (F1)

Programs (opcode census, 26177 programs, 2026-11-28): `legacyF32ToF16` in the DXIL of 11651; by ISA site, programs: `v_cvt_f16_f32` 5976 (42797
instructions), `v_fma_mixlo/hi_f16` 5816 (34109), `v_interp_p2_f16` 4752 ps (118404 `v_interp_*_f16`), VOP f16 arithmetic 171 (3344), `v_pk_*_f16` 83;
`v_cvt_pkrtz_f16_f32` 10467 is round-toward-zero by definition (unchanged); 65 cs d16 stores are format conversions (unchanged). Union of the programs with a
nearest-sensitive result: 11642. Before this work `VICTORY_F16_NEAREST=1` covered the VOP f16 and `v_cvt_f16_f32` results only; fma_mix and interp_p2 used the host
conversion. F1 routes all of them through one helper and turns it on by default.

Checks (Apple M5 Max, MSC 4.0.3, 2026-11-28):
* GPU, the translator's own DXIL (victory-shader `emit/f16_hw_tests.rs` on the mac executor host, branch `mac/f16-hw-proof`): `v_cvt_f16_f32`, `v_fma_mixlo_f16` (f32
  operands) and `v_add_f16` (pair of halves) against the exact CPU nearest-even rounding, 111 of the 256 chunks of 2^24 inputs (1.87e9, every exponent range of both signs, denormals,
  65520 overflow, Inf, NaN, -0): 0 differences. The same programs with the switch off differ for 50,329,600 of 134,217,728 sampled inputs (37 %). The untested chunks are
  the other ranges of the same exponents (`VICTORY_F16_CHUNKS=0-255` runs them all, in segments).
* Census translate with the switch off and on (25985 containers): 25760 translated, 225 failed, the same programs; 11621 translations change (cs 989, vs 749, ms 4745,
  ps 5138); their DXIL grows 574.4 MB -> 599.7 MB (+4.4 %; median +6 %, p90 +14 %, max 2.9x).
* MSC and pipeline on a 20 % sample of the changed programs (2325): cs 198/198, vs 165/165, ps 104/1028, ms 917/934, the same programs on and off. Pipeline build
  time of the sample, on a loaded machine: cs +57 %, vs +17 %, ps +59 %, ms +43 %. GPU run time of the extra ~12 operations per result: not measured.

Result: correct, pass counts unchanged, cost is DXIL size and pipeline build time (not zero). Delivered as `mac: F1` on branch `mac/f16-nearest` (default on, `=0` off).

## Typed UAV additional formats, ROV

Typed UAV load: all 21 formats D3D12 lists for `TypedUAVLoadAdditionalFormats` (+5 more: R32G32_FLOAT, R16G16_FLOAT/UNORM, R8G8_UNORM/UINT)
load exactly the CPU-decoded values; missing channels read (0,0,0,1).
ROV: 64 full-screen triangles do an order-dependent read-modify-write of one R32_UINT texel; the `RasterizerOrderedTexture2D` result equals
the in-order CPU result in 640 of 640 pixel runs (10 runs of 64 pixels); the same shader on a plain UAV is wrong in 64 of 64. MSC maps ROVs to
raster order groups.

## Not done / remaining

* `dot4AddPacked` (13 cs) and `textureStore.i32` (1 cs): closed in the second batch (`dxop_dot4_store.rs`; S-D-016, S-D-017).
* The 62 programs that convert for i64 were converted, not run: their runs belong to the mac-host-harness line (shader-exec), noted in S-D-012.
* S-D rows for dx.op groups not in this node list (sample.f32 in cs, rawBufferLoad etc.) are not added.
