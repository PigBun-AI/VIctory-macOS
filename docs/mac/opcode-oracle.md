# The per-mnemonic opcode oracle (`inv.isa.opcode_oracle`)

*Patch S6, branch `mac/shader-harness`. Code: `crates/victory-shader/src/emit/oracle_tests.rs`. Run: `tools/mac/shader-hw.sh` (or `cargo test -p victory-shader --lib oracle_` with `VICTORY_SHADER_EXEC` set).*

## What it does

The census (`research/map-plan/isa-mnemonics.tsv`, 430 mnemonics) lists 392 mnemonics the game uses and no test names (`test_ref = -`; 40,192,839 of 44,048,354 instructions). The oracle is a table: one row per mnemonic, a CPU reference, an input family (`float` or `int`). For a row the instruction is built **from its name** (`gfx10::opcode_of`, the same Mesa tables the decoder uses, so a row cannot name a mnemonic the decoder does not know), put into a one-thread compute shader (`valu_tests::build_valu_cs`: root constants in `s0..s3` and `v1..v4`), translated, run on the device (Metal through the executor on a Mac, D3D12 on Windows) for 220 to 730 input sets, and compared with the reference. Inputs: every pair of 21 integer or 24 float special values (0, -0, 1, 0.5, 3.5, 1e-20, 1e20, max, inf, NaN, subnormals, 2^23, 2^24, byte and shift edges ...), plus pseudo-random ones; three-operand rows use 400 mixed triples.

Comparison: floats: NaN equals any NaN; a subnormal equals zero or a subnormal (the console's denormal mode is open); zeros keep their sign; else within the row's units of the last place (0 for add/mul/fma/cvt/round, 2 for rcp/rsq/sqrt, 4 for exp/log, plus 2e-6 absolute for log near 1). Each float row accepts the reference of the raw and of the subnormal-flushed inputs. A fused and an unfused result are both accepted for `v_mad_f32`/`v_mac_f32`.

An outcome is *pass*, *MISMATCH*, *UNTRANSLATED* (the translator refused or panicked: a finding), or *refused* (the device converter does not take it: skipped). A MISMATCH or UNTRANSLATED row fails its family test unless `KNOWN` lists it with a reason. `VICTORY_ORACLE_OUT=<file>` appends one tab-separated line per row.

## The first ten families (done)

| family (test) | rows |
|---|---|
| `valu_f32_arith` | v_add/sub/subrev/mul/mul_legacy/min/max/fma/fmac/mad/mac_f32 (11) |
| `valu_move_select` | v_mov_b32, v_readfirstlane_b32, v_cndmask_b32 (3) |
| `valu_int_arith` | add/sub/subrev_nc_u32, mul_lo/hi_u32, mul_hi_i32, mul_u32_u24, mul_i32_i24, mad_u32_u24, mad_i32_i24, min/max u32 i32 (14) |
| `valu_bit_ops` | and/or/xor/not, lshrrev/lshlrev/ashrrev, bfe u32 i32, bfi, alignbit, bcnt, ffbh, ffbl, bfrev, lshl_or, and_or, or3 (18) |
| `valu_trans_round` | rcp, rsq, sqrt, exp, log, floor, ceil, trunc, rndne (9) |
| `valu_cvt` | f32_u32, f32_i32, u32_f32, i32_f32, flr_i32_f32, rpi_i32_f32, f32_ubyte0..3, f32_f16, f16_f32 (12) |
| `vcmp_f32` | the 16 `v_cmp_*_f32` |
| `vcmp_int` | the 16 `v_cmp_*_u32` / `_i32` |
| `salu_alu` | s_mov, not, and, or, xor, andn2, orn2, nand, nor, xnor, lshl, lshr, ashr, add/sub u32 i32, mul_i32, min/max, abs, bcnt1, ff1, flbit, bfe_u32, lshl1_add (28) |
| `salu_cmp_select` | the 12 `s_cmp_*` and s_cselect_b32 |

116 of the 392 untested mnemonics are now named by a test: **28.6 of the 40.2 million untested instructions (71 %)**; with the ones already named, the oracle touches 31.1 of 44.0 million instructions (71 %).

## Result on the Mac GPU (Apple, wave32, MSC 4.0.3, executor compat BOUNDS_CHECK | DISABLE_NAN_INF)

Every row passes except `v_cvt_f16_f32` (a documented translator default, see below). The first run showed four wrong references (not translator or Metal faults): `v_readfirstlane_b32` writes an SGPR; `v_cvt_rpi_i32_f32` adds 0.5 in f32 (the reference had added in f64); `v_bfe_i32` shifts arithmetically; `v_cndmask_b32` was put in with an unsupported write of `vcc` in a 1-thread program (found: `s_mov_b32 vcc_lo, s2` in a program with a thread count asserts in the lowering, `self.s.threads.is_none() || self.console_lanes`: **translator limitation, not fixed**: a thread-limited program that writes VCC by a scalar move does not translate).

- `v_cvt_f16_f32`: 104 of 224 cases differ from round-to-nearest-even by one unit toward zero (and overflow gives 0x7bff, not infinity). The translator takes the host's conversion by default (rounds toward zero on this device) and rounds to nearest only with `VICTORY_F16_NEAREST=1` (documented in `lower.rs`, a "shortcut"); with the flag all 224 cases agree (`oracle_f16_nearest_matches`). Listed in `KNOWN`. Whether the console rounds to nearest here depends on its MODE register, as the comment says.

## What the first ten families do not cover

Subnormal inputs and results are compared as equivalent (the console's mode is not known). Every row ran with all lanes active, one thread. VCC/EXEC/SCC state outside the instruction, wave64 and clamp/omod/abs/neg modifiers are not exercised. 

## A batch (VALU remainder, exec and 64-bit scalar, transcendentals): `emit/oracle_a_tests.rs`

161 mnemonics (157 of the title's: 3.77 M instructions, 8.5 % of all), 406 rows on the Mac GPU, run with `tools/mac/shader-hw.sh --filter oracle_a_`; the 16-bit float rows run with `VICTORY_F16_NEAREST=1` (the host's rounding is toward zero; `oracle_a_f16_default_rounding_report` lists what differs by default and never fails). New in the method: rows with their own program (`Spec`: literal, VOP3P, VOP3B, `M0`, carry in `vcc`; a mnemonic may have several rows: `[tag]` for the operands, `#lo`/`#hi`/`#carry` for the result of two), 16-bit-float and mixed-precision references (`to_half64`, `half_ok`: NaN equals NaN, a subnormal equals zero), and the lane family (`LaneRow`: 32 threads in one wave, exec set from a root constant, every lane's result read back) for `v_cmpx_*` (30 forms), `s_and/or_saveexec_b32`, `s_andn2_wrexec_b32`, `v_mbcnt_lo/hi`, `v_writelane_b32`, `v_permlanex16_b32`. References: the RDNA2 ISA text, Mesa/ACO semantics, the OpenGL face table for `v_cube*`.

* Multiply-add that rounds twice: `v_mad_f32`, `v_mac_f32`, `v_mad_legacy_f32`, `v_mac_legacy_f32`, `v_madak_f32`, `v_madmk_f32` are strict (a fused result fails); the inputs (`Ty::Mad`, `Ty::Sq`) are chosen so that fused and double-rounded differ (a CPU check in `every_a_row_is_a_gfx10_mnemonic` proves it). The device result is not fused: MSC keeps `fmul` + `fadd` apart.
* Fixed in the translator (`PATCHES.md` S7): the vcc bug, `v_writelane_b32` lane masking, sin/cos of NaN and infinity, `s_wqm_*`/`s_bfm_b64` of numbers.
* Differences kept: `v_add_f64` (no 64-bit float on the device: refused), `s_andn2_wrexec_b32` with its result read as data (refused by design, `MASK_ELSEWHERE`; as an exec update it passes), `v_cvt_f16_f32` and the f16 rows by default (toward zero), `s_bfe_i32` with a field past bit 31 (the ISA text is silent; two readings accepted), `v_cvt_pk_u8_f32` rounding (the ISA leaves it undefined; nearest even, as third-party RDNA2 measurements).
* Not yet: the wave64 forms (`s_or_saveexec_b64`, `s_*_b64` as exec), `v_cmpx` with wave64 masks.

## Plan: the rest (276 mnemonics, 11,578,423 instructions), by weight, and who takes which

The first ten families are done (above). The rest is split so that two hands can work in parallel without touching the same rows. Each hand adds its own file next to `oracle_tests.rs` (`oracle_a_*.rs` / `oracle_b_*.rs`, mounted in `emit/mod.rs` under the same gate) and reuses `oracle_tests`' encoders, `Want`, `Row`, `run_family` and `KNOWN` (make what it needs `pub(super)`); it does not edit the first ten families' rows.

| family | untested | instructions | hand | first by frequency |
|---|---|---|---|---|
| hint | 8 | 3,308,752 | 后续 | s_waitcnt, s_nop, v_nop, s_inst_prefetch, s_clause, s_waitcnt_vscnt ... |
| smem | 10 | 2,646,524 | B | s_load_dwordx4, s_buffer_load_dword, s_buffer_load_dwordx4, s_load_dwordx8, s_buffer_load_dwordx2, s_buffer_load_dwordx8 ... |
| salu | 42 | 1,669,132 | A | s_mov_b64, s_and_b64, s_and_saveexec_b32, s_andn2_b64, s_movk_i32, s_cselect_b64 ... |
| valu | 43 | 1,012,090 | A | v_fma_mix_f32, v_fract_f32, v_lshl_add_u32, v_med3_f32, v_fmaak_f32, v_pack_b32_f16 ... |
| buffer | 21 | 696,397 | B | buffer_load_dword, buffer_load_dwordx4, buffer_load_format_x, buffer_load_dwordx2, buffer_load_dwordx3, tbuffer_load_format_xy ... |
| sctl | 5 | 449,220 | 后续（含图形相关控制流） | s_branch, s_cbranch_vccnz, s_cbranch_execnz, s_cbranch_vccz, s_swappc_b64 |
| vcmp | 43 | 444,160 | A | v_cmpx_eq_u32, v_cmpx_lt_f32, v_cmpx_gt_f32, v_cmp_class_f32, v_cmpx_lg_f32, v_cmpx_gt_u32 ... |
| image-sample | 12 | 364,549 | B | image_sample_lz, image_sample, image_sample_l, image_sample_d, image_sample_c_lz, image_gather4_lz ... |
| interp | 5 | 361,766 | 后续（绘制路径已就绪） | v_interp_p1_f32, v_interp_p2_f32, v_interp_p1ll_f16, v_interp_p2_f16, v_interp_mov_f32 |
| export | 1 | 132,688 | 后续（绘制路径已就绪） | exp |
| lds | 23 | 123,295 | B | ds_swizzle_b32, ds_write2_b32, ds_read2_b32, ds_wrxchg_rtn_b32, ds_read2st64_b32, ds_write2st64_b32 ... |
| trans | 9 | 91,428 | A | v_rcp_iflag_f32, v_sin_f32, v_cos_f32, v_rsq_f16, v_rcp_f16, v_sqrt_f16 ... |
| image | 10 | 72,383 | B | image_load, image_store, image_atomic_umax, image_get_resinfo, image_load_mip, image_atomic_or ... |
| cvt | 3 | 57,827 | A | v_cvt_pkrtz_f16_f32, v_cvt_pk_u8_f32, v_cvt_f32_f64 |
| lane | 2 | 39,272 | A | v_writelane_b32, v_permlanex16_b32 |
| mode | 2 | 33,878 | A | s_setreg_imm32_b32, s_setreg_b32 |
| packed | 12 | 28,950 | A | v_dot4c_i32_i8, v_dot4_i32_i8, v_pk_mul_f16, v_pk_fma_f16, v_pk_add_f16, v_pk_max_f16 ... |
| barrier | 1 | 18,424 | 后续 | s_barrier |
| flat | 8 | 11,616 | B | global_load_dword, global_load_dwordx2, global_load_dwordx3, global_load_dwordx4, global_store_dwordx4, global_load_ushort ... |
| sendmsg | 1 | 9,750 | 后续 | s_sendmsg |
| vmem-atomic | 13 | 5,606 | B | buffer_atomic_add, buffer_atomic_or, buffer_atomic_umin, buffer_atomic_umax, global_atomic_fmax, global_atomic_fmin ... |
| bvh | 2 | 716 | B | image_bvh_intersect_ray, image_bvh64_intersect_ray |

**Hand A** (ALU): VALU remainder (`v_fma_mix*`, `v_fract`, `v_med3`, `v_lshl_add`, `v_add3`, `v_fmaak/fmamk`, `v_pack_b32_f16`, `v_mad_legacy`, `v_cube*`), `v_cmpx_*` and `v_cmp_class_f32`, 64-bit and exec-state scalar ops (`s_*_b64`, `s_and_saveexec_*`, `s_wqm_b64`, `s_movk/addk`, `s_brev`), `sin/cos` and the f16 transcendentals, `cvt` leftovers (`pkrtz`, `pk_u8`, f64), `packed` (`v_pk_*`, dot), `lane` (`v_writelane`, `permlanex16`), `mode` (`s_setreg*`). Needs a multi-lane builder (`build_cs` with a thread count, result per lane; EXEC/VCC as a per-lane model). Known issue to fix first: `s_mov_b32 vcc_lo, sN` in a thread-limited program asserts in the lowering (`self.s.threads.is_none() || self.console_lanes`), found by the `v_cndmask_b32` row, which now builds vcc with `v_cmp_ne_u32` instead.

**Hand B** (memory): `s_load_*`, `s_buffer_load_*`, `buffer_*`/`tbuffer_*`, `global_*`/`flat_*`, `ds_*`, `*_atomic*`, and the images (`image_load/store/atomic/get_resinfo/get_lod`, `image_sample*`, `gather4`, `bvh`). The table is (address, offset, stride, bounds) against a CPU memory model over the host's `View`s (`Raw/Typed/Texture/Cube`), samplers in `Dispatch.samplers`. Known issues for B to judge: a texture store followed by a texture atomic of the same texel in one thread is applied after it on Metal (`a_counter_texture_is_set_by_a_store_of_one_channel`: should the translator put a fence between them?), and the vector scalar-load that straddles the end of its view (`scalar_buffer_loads…@VICTORY_SHADER_OPT=127`).

**Later** (needs the graphics path, now in the executor: ops 4 and 5): `v_interp_*`, `exp`, the control flow that only shows in a draw, `s_barrier`, `s_sendmsg`, hints (rows that translate and do not change the result). Pull-model barycentrics are refused by MSC 4.0.3 (`attributeAtVertex`), so rows needing them skip.

Next batches, in order:
1. **VALU remainder** (43 mnemonics, 1.0 M): `v_fma_mix_f32`/`v_fma_mixlo_f16` (VOP3P, op_sel_hi), `v_fract_f32`, `v_lshl_add_u32`, `v_add3_u32`, `v_med3_f32`, `v_fmaak/fmamk` (literal), `v_pack_b32_f16`, `v_mad_legacy_f32`, `v_cube*`. The same builder; VOP3P and literal encoders to add; modifier variants (abs/neg/omod/clamp as extra columns of the same rows).
2. **Lane and exec state** (`vcmp` 43 rows: `v_cmpx_*`, `v_cmp_class_f32`; salu 42: the `_b64`, `s_and_saveexec_*`, `s_wqm_b64`, `s_brev`, `s_movk/addk`): a multi-lane variant of the builder (`build_cs` with 64 threads, result per lane, `wave64` on and off); the reference is a per-lane model of EXEC/VCC. Where the wave64 on wave32 lowering is under test the Mac GPU is the only place it can run (MSC takes wave32 only).
3. **Transcendentals**: `v_sin/cos_f32` (the known NaN difference: `v_sin_and_v_cos_f32_reduce_large_turns` fails on the Mac GPU), `v_rcp_iflag_f32`, the f16 forms; ulps measured, not guessed.
4. **Memory**: `s_load_*`, `s_buffer_load_*` (2.6 M), `buffer_load/store*`, `tbuffer_*`, `global_*`, `ds_*`, `buffer_atomic*`: one table of (address, offset, stride, bounds) against a CPU memory model, over the existing `View::Buffer/RawUav/TypedUav`, including the range-check cases of `scalar_buffer_loads_read_the_same_dwords_one_call_or_four` (a known Metal difference: a vector load that straddles the end of the view reads the next memory).
5. **Images** (`image_sample*`, `gather4`, `image_load/store/atomic`, 0.4 M): texel sets with the host's `View::Texture/Cube` and samplers; the reference is a CPU sampler per filter/address mode (a tolerance for the filter weights, as on any GPU).
6. **Control flow, interpolation, export, hints, `s_barrier`, `s_setreg`, `s_sendmsg`, bvh**: not numeric; `s_branch`/`s_cbranch_*` by structured programs of the existing `structure` tests (a branch table); `v_interp_*` and `exp` need the graphics path of the host (phase B); hints (`s_waitcnt`, `s_nop`, `v_nop`, `s_clause` ...) as "translates and does not change the result" rows.

## Honest limits

The oracle checks the translator and the device against a CPU model of the ISA text; it is not a console. Where the ISA text and the console differ (denormal mode, rounding mode registers, out-of-range fields) a row records what the translator does and a person decides.
