//! mac S6, oracle batch B: the data share (LDS) instructions, the cross-lane ones through the LDS
//! hardware (`ds_swizzle_b32`, `ds_bpermute_b32`) and the buffer atomics (`buffer_atomic_*`).
//!
//! References: GFX10 (RDNA 2) ISA manual, "Data Share Instructions" (the address is a byte
//! address, `offset0`/`offset1` count dwords (x2) or 64-dword blocks (x2st64) of the element
//! size; `ds_cmpst`: `mem = (mem == data0) ? data1 : mem`, returns the old value; `ds_swizzle_b32`:
//! offset[15] set: quad permute, four 2-bit lane selects; else bit mode, and_mask = [4:0],
//! or_mask = [9:5], xor_mask = [14:10] on the lane index within 32; `ds_bpermute_b32`: lane i
//! reads the data register of lane `((addr + offset) / 4) mod wave size`), Mesa's `ac_nir` LDS
//! lowering for the 2- and 64-block forms, and the D3D12 specification for `InterlockedXxx` on
//! UAVs (the atomics). The LDS model is exact: LDS accesses are taken to be in bounds (a shortcut
//! of the translator, see `lower.rs` `ds`), so no row goes out of the allocation.

use super::host::Host;
use super::host::View;
use super::oracle_mem_tests::{
    Buf, Cfg, Srv, merge, mubuf, opc, pattern, store_out, sweep, try_build, v_mov_lit,
};
use super::oracle_tests::{Outcome, report_family};
use super::valu_tests::vop1;

/// DS `name vdst, vaddr, vdata0, vdata1 offset0 offset1`.
fn ds(name: &str, vdst: u8, addr: u8, d0: u8, d1: u8, off0: u8, off1: u8) -> [u32; 2] {
    [
        0x36 << 26 | opc(name) << 18 | u32::from(off1) << 8 | u32::from(off0),
        u32::from(vdst) << 24 | u32::from(d1) << 16 | u32::from(d0) << 8 | u32::from(addr),
    ]
}

/// `ds name` with one 16-bit offset.
fn ds16(name: &str, vdst: u8, addr: u8, d0: u8, d1: u8, off: u16) -> [u32; 2] {
    ds(name, vdst, addr, d0, d1, off as u8, (off >> 8) as u8)
}

const fn vop2(op: u16, vdst: u8, src0: u16, vsrc1: u8) -> u32 {
    (op as u32) << 25 | (vdst as u32) << 17 | (vsrc1 as u32) << 9 | (src0 as u32 & 0x1FF)
}

/// LDS blocks the sequential rows initialise and dump: dwords 0..16 and 64..80.
const BLOCKS: [usize; 2] = [0, 64];
const BLOCK: usize = 16;

fn lds_init() -> Vec<u32> {
    let p = pattern(2 * BLOCK);
    p
}

/// A one-thread program: fill the blocks, `op`, dump the blocks and the four result registers
/// v12..v15 (preset to `0xCCCC_000k`).
fn seq_program(op: &[u32]) -> Vec<u32> {
    let init = lds_init();
    let mut w = vec![vop1(0x001, 5, 0x80)]; // v5 = 0, the LDS base
    for (bi, &base) in BLOCKS.iter().enumerate() {
        for k in 0..BLOCK {
            w.extend(v_mov_lit(6, init[bi * BLOCK + k]));
            w.extend(ds16("ds_write_b32", 0, 5, 6, 0, (4 * (base + k)) as u16));
        }
    }
    for k in 0..4u8 {
        w.extend(v_mov_lit(12 + k, 0xCCCC_0000 + u32::from(k)));
    }
    w.extend_from_slice(op);
    let mut out = 0u16;
    for &base in &BLOCKS {
        for k in 0..BLOCK {
            w.extend(ds16("ds_read_b32", 20, 5, 0, 0, (4 * (base + k)) as u16));
            w.extend(store_out(20, 4 * out));
            out += 1;
        }
    }
    for k in 0..4u8 {
        w.extend(store_out(12 + k, 4 * out));
        out += 1;
    }
    w
}

/// The model's memory: 128 dwords, the blocks initialised.
struct Lds([u32; 128]);

impl Lds {
    fn new() -> Self {
        let init = lds_init();
        let mut m = [0u32; 128];
        for (bi, &base) in BLOCKS.iter().enumerate() {
            m[base..base + BLOCK].copy_from_slice(&init[bi * BLOCK..(bi + 1) * BLOCK]);
        }
        Lds(m)
    }
    fn dump(&self, dst: [u32; 4]) -> Vec<u32> {
        let mut v = Vec::new();
        for &base in &BLOCKS {
            v.extend_from_slice(&self.0[base..base + BLOCK]);
        }
        v.extend_from_slice(&dst);
        v
    }
}

struct SeqRow {
    name: &'static str,
    /// The instruction(s): the address is v1 (c0), data v2 (c1), v3 (c2), v4 (c3), v5 is 0.
    inst: Vec<u32>,
    /// Applies the instruction on the model; the result registers v12..v15.
    model: Box<dyn Fn(&mut Lds, [u32; 4], &mut [u32; 4])>,
    cases: Vec<[u32; 4]>,
}

fn run_seq(row: &SeqRow) -> Outcome {
    let mut cfg = Cfg::new(Srv::No);
    cfg.lds_bytes = 1024;
    cfg.vgprs = 32;
    let shader = match try_build(&seq_program(&row.inst), &cfg) {
        Ok(s) => s,
        Err(o) => return o,
    };
    sweep(&shader, &[View::RawUav(64)], &[], &row.cases, |c, got| {
        let mut lds = Lds::new();
        let mut dst = [0xCCCC_0000, 0xCCCC_0001, 0xCCCC_0002, 0xCCCC_0003];
        (row.model)(&mut lds, c, &mut dst);
        let want = lds.dump(dst);
        (got[0][..want.len()] != want[..]).then(|| {
            let at = want.iter().zip(&got[0]).position(|(a, b)| a != b).unwrap_or(0);
            format!("c {c:x?}: first difference at out dword {at}: got {:#x} want {:#x}", got[0][at], want[at])
        })
    })
}

fn dword_cases(step: u32, n: u32) -> Vec<[u32; 4]> {
    (0..n).map(|i| [i * step, 0x1357_9BDF, 0x2468_ACE0, 0x0F0F_F0F0]).collect()
}

fn seq_rows() -> Vec<SeqRow> {
    let w32 = dword_cases(4, 6);
    let w64 = dword_cases(8, 4);
    let w16: Vec<[u32; 4]> = (0..8).map(|i| [2 * i, 0x1357_9BDF, 0x2468_ACE0, 0x0F0F_F0F0]).collect();
    let at = |c: [u32; 4]| (c[0] / 4) as usize;
    vec![
        SeqRow { name: "ds_write_b32", inst: ds16("ds_write_b32", 0, 1, 2, 0, 8).to_vec(), cases: w32.clone(), model: Box::new(move |m, c, _| m.0[at(c) + 2] = c[1]) },
        SeqRow { name: "ds_write_b64", inst: ds16("ds_write_b64", 0, 1, 2, 0, 4).to_vec(), cases: w64.clone(), model: Box::new(move |m, c, _| {
            m.0[at(c) + 1] = c[1];
            m.0[at(c) + 2] = c[2];
        }) },
        SeqRow { name: "ds_write_b128", inst: {
            let mut x = vec![];
            x.push(vop1(0x001, 8, 0x102)); // v8 = c1
            x.push(vop1(0x001, 9, 0x103)); // v9 = c2
            x.push(vop1(0x001, 10, 0x104)); // v10 = c3
            x.extend(v_mov_lit(11, 0xE5E5_0005));
            x.extend(ds16("ds_write_b128", 0, 1, 8, 0, 0));
            x
        }, cases: dword_cases(16, 2), model: Box::new(move |m, c, _| {
            m.0[at(c)] = c[1];
            m.0[at(c) + 1] = c[2];
            m.0[at(c) + 2] = c[3];
            m.0[at(c) + 3] = 0xE5E5_0005;
        }) },
        SeqRow { name: "ds_write2_b32", inst: ds("ds_write2_b32", 0, 1, 2, 3, 1, 3).to_vec(), cases: w32.clone(), model: Box::new(move |m, c, _| {
            m.0[at(c) + 1] = c[1];
            m.0[at(c) + 3] = c[2];
        }) },
        SeqRow { name: "ds_write2_b64", inst: {
            // data0 = v[2:3] (c1, c2), data1 = v[8:9]
            let mut w = v_mov_lit(8, 0xAAAA_0008).to_vec();
            w.extend(v_mov_lit(9, 0xBBBB_0009));
            w.extend(ds("ds_write2_b64", 0, 1, 2, 8, 1, 3));
            w
        }, cases: dword_cases(8, 2), model: Box::new(move |m, c, _| {
            // offsets count 8-byte elements: dword 2 and 6 from the address
            m.0[at(c) + 2] = c[1];
            m.0[at(c) + 3] = c[2];
            m.0[at(c) + 6] = 0xAAAA_0008;
            m.0[at(c) + 7] = 0xBBBB_0009;
        }) },
        SeqRow { name: "ds_write2st64_b32", inst: ds("ds_write2st64_b32", 0, 1, 2, 3, 0, 1).to_vec(), cases: w32.clone(), model: Box::new(move |m, c, _| {
            // the second offset is 1 block of 64 dwords
            m.0[at(c)] = c[1];
            m.0[at(c) + 64] = c[2];
        }) },
        SeqRow { name: "ds_write_b16", inst: ds16("ds_write_b16", 0, 1, 2, 0, 0).to_vec(), cases: w16.clone(), model: Box::new(move |m, c, _| {
            let (w, sh) = ((c[0] / 4) as usize, 8 * (c[0] & 2));
            m.0[w] = m.0[w] & !(0xFFFF << sh) | (c[1] & 0xFFFF) << sh;
        }) },
        SeqRow { name: "ds_write_b16_d16_hi", inst: ds16("ds_write_b16_d16_hi", 0, 1, 2, 0, 0).to_vec(), cases: w16.clone(), model: Box::new(move |m, c, _| {
            let (w, sh) = ((c[0] / 4) as usize, 8 * (c[0] & 2));
            m.0[w] = m.0[w] & !(0xFFFF << sh) | (c[1] >> 16) << sh;
        }) },
        SeqRow { name: "ds_read_b32", inst: ds16("ds_read_b32", 12, 1, 0, 0, 4).to_vec(), cases: w32.clone(), model: Box::new(move |m, c, d| d[0] = m.0[at(c) + 1]) },
        SeqRow { name: "ds_read_b64", inst: ds16("ds_read_b64", 12, 1, 0, 0, 0).to_vec(), cases: w64.clone(), model: Box::new(move |m, c, d| {
            d[0] = m.0[at(c)];
            d[1] = m.0[at(c) + 1];
        }) },
        SeqRow { name: "ds_read2_b32", inst: ds("ds_read2_b32", 12, 1, 0, 0, 1, 3).to_vec(), cases: w32.clone(), model: Box::new(move |m, c, d| {
            d[0] = m.0[at(c) + 1];
            d[1] = m.0[at(c) + 3];
        }) },
        SeqRow { name: "ds_read2st64_b32", inst: ds("ds_read2st64_b32", 12, 1, 0, 0, 0, 1).to_vec(), cases: w32.clone(), model: Box::new(move |m, c, d| {
            d[0] = m.0[at(c)];
            d[1] = m.0[at(c) + 64];
        }) },
        SeqRow { name: "ds_read_u16", inst: ds16("ds_read_u16", 12, 1, 0, 0, 0).to_vec(), cases: w16.clone(), model: Box::new(move |m, c, d| {
            d[0] = (m.0[(c[0] / 4) as usize] >> (8 * (c[0] & 2))) & 0xFFFF;
        }) },
        SeqRow { name: "ds_read_u16_d16_hi", inst: ds16("ds_read_u16_d16_hi", 12, 1, 0, 0, 0).to_vec(), cases: w16.clone(), model: Box::new(move |m, c, d| {
            let half = (m.0[(c[0] / 4) as usize] >> (8 * (c[0] & 2))) & 0xFFFF;
            d[0] = d[0] & 0xFFFF | half << 16;
        }) },
    ]
}

fn lds_sequential() -> Vec<(String, Outcome)> {
    seq_rows().iter().map(|r| (r.name.to_string(), run_seq(r))).collect()
}

// ---- LDS atomics: one thread, the memory dword 0 holds c3, data c1 (and c2) ----

fn atomic_cases() -> Vec<[u32; 4]> {
    let sp = [0u32, 1, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0x1234_5678, 7, 0xF0F0_F0F0];
    let mut v = Vec::new();
    for &a in &sp {
        for &b in &sp {
            for &m in &sp {
                v.push([0, a, b, m]);
            }
        }
    }
    v
}

fn lds_atomic_rows() -> Vec<(String, Outcome)> {
    type Op = fn(mem: u32, d0: u32, d1: u32) -> u32;
    let rows: [(&'static str, bool, Op); 9] = [
        ("ds_add_u32", false, |m, a, _| m.wrapping_add(a)),
        ("ds_add_rtn_u32", true, |m, a, _| m.wrapping_add(a)),
        ("ds_max_u32", false, |m, a, _| m.max(a)),
        ("ds_min_u32", false, |m, a, _| m.min(a)),
        ("ds_or_b32", false, |m, a, _| m | a),
        ("ds_and_b32", false, |m, a, _| m & a),
        ("ds_max_i32", false, |m, a, _| (m as i32).max(a as i32) as u32),
        ("ds_wrxchg_rtn_b32", true, |_, a, _| a),
        ("ds_cmpst_rtn_b32", true, |m, a, b| if m == a { b } else { m }),
    ];
    rows.into_iter()
        .map(|(name, rtn, f)| {
            // v5 = 0; LDS[0] = v4 (c3); the atomic on v5 with v2 (c1), v3 (c2); v12 = old (rtn);
            // v13 = LDS[0] afterwards; both to the UAV.
            let mut w = vec![vop1(0x001, 5, 0x80)];
            w.extend(ds16("ds_write_b32", 0, 5, 4, 0, 0));
            w.extend(ds16(name, if rtn { 12 } else { 0 }, 5, 2, 3, 0));
            w.extend(ds16("ds_read_b32", 13, 5, 0, 0, 0));
            w.extend(store_out(12, 0));
            w.extend(store_out(13, 4));
            let mut cfg = Cfg::new(Srv::No);
            cfg.lds_bytes = 512;
            cfg.vgprs = 32;
            let o = match try_build(&w, &cfg) {
                Err(o) => o,
                Ok(shader) => sweep(&shader, &[View::RawUav(4)], &[], &atomic_cases(), |c, got| {
                    let new = f(c[3], c[1], c[2]);
                    let want = [if rtn { c[3] } else { 0 }, new];
                    (got[0][..2] != want).then(|| format!("mem {:#x} d0 {:#x} d1 {:#x}: got {:x?} want {want:x?}", c[3], c[1], c[2], &got[0][..2]))
                }),
            };
            (name.to_string(), o)
        })
        .collect()
}

// ---- 32 threads: swizzle, bpermute, bank patterns ----

/// A program of 32 threads: v6 = tid * 4 (the output offset), v7 = (tid + 1) * 0x01010101, then
/// `op` (the source in v7, result in v8), `buffer_store_dword v8, v6 offen`.
fn lane_program(op: &[u32]) -> Vec<u32> {
    let mut w = vec![
        vop2(0x1A, 6, 130, 0), // v_lshlrev_b32 v6, 2, v0   (130 = the inline constant 2)
        vop2(0x25, 9, 129, 0), // v_add_nc_u32 v9, 1, v0
    ];
    w.extend(v_mov_lit(10, 0x0101_0101));
    // v7 = v9 * v10 (low 32 bits): v_mul_lo_u32 is VOP3; the 24-bit multiply is enough here
    w.extend(super::valu_tests::vop3(opc_vop3("v_mul_u32_u24"), 7, 256 + 9, 256 + 10, 0));
    w.extend_from_slice(op);
    w.extend(mubuf(
        "buffer_store_dword",
        Buf { vdata: 8, vaddr: 6, srsrc: 12, soffset: 128, offset: 0, offen: true, idxen: false, glc: false },
    ));
    w
}

fn opc_vop3(name: &str) -> u16 {
    // the VOP2 opcode of `name` lives in VOP3 at 0x100 + op
    let o = opc(name) as u16;
    if o < 0x100 { 0x100 + o } else { o }
}

fn lane_value(l: u32) -> u32 {
    ((l + 1) & 0xFF_FFFF).wrapping_mul(0x0101_0101 & 0xFF_FFFF)
}

fn swizzle_rows() -> Vec<(String, Outcome)> {
    // quad permutes (offset bit 15) and bit masks (and | or << 5 | xor << 10)
    let patterns: Vec<u16> = vec![
        0x8000, 0x80B1, 0x804E, 0x80E4, 0x801B, 0x8055, 0x80FF, 0x80AA, 0x8044, // quad
        0x001F, 0x041F, 0x081F, 0x101F, 0x201F, 0x401F, 0x7C1F, // xor masks (5 bits, 31 = all)
        0x0010, 0x0001, 0x03E0, 0x07FF, 0x0C1E, 0x1C1F, 0x5BEF, // or / and mixes
    ];
    let mut parts = Vec::new();
    for off in patterns {
        let mut cfg = Cfg::new(Srv::No);
        cfg.threads = 32;
        let mut body = ds16("ds_swizzle_b32", 8, 7, 0, 0, off).to_vec();
        let words = lane_program(&body);
        body.clear();
        let shader = match try_build(&words, &cfg) {
            Ok(s) => s,
            Err(o) => {
                parts.push((format!("offset {off:#06x}"), o));
                continue;
            }
        };
        let o = sweep(&shader, &[View::RawUav(32)], &[], &[[0; 4]], |_, got| {
            for l in 0..32u32 {
                let src = if off & 0x8000 != 0 {
                    (l & !3) | ((u32::from(off) >> (2 * (l & 3))) & 3)
                } else {
                    let (and, or, xor) = (u32::from(off) & 0x1F, u32::from(off) >> 5 & 0x1F, u32::from(off) >> 10 & 0x1F);
                    ((l & and) | or) ^ xor
                };
                let want = lane_value(src & 31);
                if got[0][l as usize] != want {
                    return Some(format!("lane {l}: got {:#x} want {want:#x} (lane {src})", got[0][l as usize]));
                }
            }
            None
        });
        parts.push((format!("offset {off:#06x}"), o));
    }
    vec![("ds_swizzle_b32".to_string(), merge(parts))]
}

fn bpermute_rows() -> Vec<(String, Outcome)> {
    let mut parts = Vec::new();
    for imm in [0u16, 4, 12] {
        let mut cfg = Cfg::new(Srv::No);
        cfg.threads = 32;
        // v11 = (tid ^ c1) * 4: the lane to read, as a byte address
        let mut op = vec![vop2(0x1D, 11, 258, 0)]; // v_xor_b32 v11, v2, v0  (v2 = c1)
        op.push(vop2(0x1A, 11, 130, 11)); // v_lshlrev_b32 v11, 2, v11
        op.extend(ds16("ds_bpermute_b32", 8, 11, 7, 0, imm));
        let words = lane_program(&op);
        let shader = match try_build(&words, &cfg) {
            Ok(s) => s,
            Err(o) => {
                parts.push((format!("offset {imm}"), o));
                continue;
            }
        };
        let cases: Vec<[u32; 4]> = [0u32, 1, 2, 3, 5, 8, 16, 31, 33].iter().map(|&x| [0, x, 0, 0]).collect();
        let o = sweep(&shader, &[View::RawUav(32)], &[], &cases, |c, got| {
            for l in 0..32u32 {
                let from = (((l ^ c[1]) * 4 + u32::from(imm)) / 4) & 31;
                let want = lane_value(from);
                if got[0][l as usize] != want {
                    return Some(format!("c1 {}: lane {l}: got {:#x} want {want:#x} (lane {from})", c[1], got[0][l as usize]));
                }
            }
            None
        });
        parts.push((format!("offset {imm}"), o));
    }
    vec![("ds_bpermute_b32".to_string(), merge(parts))]
}

/// The LDS under access patterns that hit one bank (32 banks of 4 bytes): each of 64 threads
/// writes its dword at `tid * stride` (stride 1, 2, 4, 32 (all the same bank), 33), the group
/// meets at a barrier, and each thread reads the dword of the thread `tid + 1` wrote. The
/// results are the values, whatever the bank conflicts cost.
fn bank_patterns() -> Vec<(String, Outcome)> {
    let mut parts = Vec::new();
    for stride in [1u32, 2, 4, 32, 33] {
        let mut cfg = Cfg::new(Srv::No);
        cfg.threads = 64;
        cfg.lds_bytes = 9216;
        cfg.vgprs = 32;
        let barrier = 0xBF8A_0000u32; // s_barrier
        let mut w = vec![
            vop2(0x1A, 6, 130, 0), // v6 = tid * 4 (output)
            vop2(0x25, 9, 129, 0), // v9 = tid + 1
        ];
        w.extend(v_mov_lit(10, stride * 4));
        // v11 = tid * stride * 4; v12 = ((tid + 1) & 63) * stride * 4
        w.extend(super::valu_tests::vop3(opc_vop3("v_mul_u32_u24"), 11, 256, 256 + 10, 0));
        w.extend(v_mov_lit(13, 63));
        w.extend(super::valu_tests::vop3(opc_vop3("v_and_b32"), 12, 256 + 9, 256 + 13, 0));
        w.extend(super::valu_tests::vop3(opc_vop3("v_mul_u32_u24"), 12, 256 + 12, 256 + 10, 0));
        // v14 = (tid + 1) * 7 + 5: the value the thread writes
        w.extend(v_mov_lit(15, 7));
        w.extend(super::valu_tests::vop3(opc_vop3("v_mul_u32_u24"), 14, 256 + 9, 256 + 15, 0));
        w.extend(ds16("ds_write_b32", 0, 11, 14, 0, 0));
        w.push(barrier);
        w.extend(ds16("ds_read_b32", 8, 12, 0, 0, 0));
        w.extend(mubuf(
            "buffer_store_dword",
            Buf { vdata: 8, vaddr: 6, srsrc: 12, soffset: 128, offset: 0, offen: true, idxen: false, glc: false },
        ));
        let shader = match try_build(&w, &cfg) {
            Ok(s) => s,
            Err(o) => {
                parts.push((format!("stride {stride}"), o));
                continue;
            }
        };
        let o = sweep(&shader, &[View::RawUav(64)], &[], &[[0; 4]], |_, got| {
            // thread t reads the dword at ((t + 1) & 63) * stride, which thread (t + 1) & 63
            // wrote: its value ((t + 1) & 63 + 1) * 7
            for t in 0..64u32 {
                let writer = (t + 1) & 63;
                let want = (writer + 1) * 7;
                if got[0][t as usize] != want {
                    return Some(format!("thread {t}: got {} want {want}", got[0][t as usize]));
                }
            }
            None
        });
        parts.push((format!("stride {stride}"), o));
    }
    vec![("ds_write_b32".to_string(), merge(parts))]
}

// ---- buffer atomics (raw UAV) ----

fn buffer_atomic_rows() -> Vec<(String, Outcome)> {
    type Op = fn(mem: u32, data: u32) -> u32;
    let rows: [(&'static str, Op); 8] = [
        ("buffer_atomic_add", |m, d| m.wrapping_add(d)),
        ("buffer_atomic_or", |m, d| m | d),
        ("buffer_atomic_and", |m, d| m & d),
        ("buffer_atomic_umin", |m, d| m.min(d)),
        ("buffer_atomic_umax", |m, d| m.max(d)),
        ("buffer_atomic_smin", |m, d| (m as i32).min(d as i32) as u32),
        ("buffer_atomic_smax", |m, d| (m as i32).max(d as i32) as u32),
        ("buffer_atomic_swap", |_, d| d),
    ];
    let sp = [0u32, 1, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0x1234_5678, 7, 0xF0F0_F0F0];
    let cases: Vec<[u32; 4]> = sp.iter().flat_map(|&m| sp.iter().map(move |&d| [m, d, 0, 0])).collect();
    let mut out: Vec<(String, Outcome)> = rows
        .into_iter()
        .map(|(name, f)| {
            // UAV[0] = c0 (v1); the atomic with c1 (v2) returns the old value in v8; old to dword 1
            let uav = |off: u16| Buf { vdata: 8, vaddr: 0, srsrc: 12, soffset: 128, offset: off, offen: false, idxen: false, glc: false };
            let mut w = vec![vop1(0x001, 8, 0x102)]; // v8 = v2 (the operand)
            w.extend(mubuf("buffer_store_dword", Buf { vdata: 1, ..uav(0) }));
            w.extend(mubuf(name, Buf { glc: true, ..uav(0) }));
            w.extend(store_out(8, 4));
            let o = match try_build(&w, &Cfg::new(Srv::No)) {
                Err(o) => o,
                Ok(shader) => sweep(&shader, &[View::RawUav(4)], &[], &cases, |c, got| {
                    let want = [f(c[0], c[1]), c[0]];
                    (got[0][..2] != want).then(|| format!("mem {:#x} data {:#x}: got {:x?} want {want:x?}", c[0], c[1], &got[0][..2]))
                }),
            };
            (name.to_string(), o)
        })
        .collect();
    // compare-swap: data (new, compare) in v[8:9]; returns the old value
    let cs = {
        let uav = |off: u16| Buf { vdata: 8, vaddr: 0, srsrc: 12, soffset: 128, offset: off, offen: false, idxen: false, glc: false };
        let mut w = vec![vop1(0x001, 8, 0x103), vop1(0x001, 9, 0x102)]; // v8 = new (c2), v9 = compare (c1)
        w.extend(mubuf("buffer_store_dword", Buf { vdata: 1, ..uav(0) }));
        w.extend(mubuf("buffer_atomic_cmpswap", Buf { glc: true, ..uav(0) }));
        w.extend(store_out(8, 4));
        let cases: Vec<[u32; 4]> = sp.iter().flat_map(|&m| sp.iter().map(move |&k| [m, k, 0xABCD_0123, 0])).collect();
        match try_build(&w, &Cfg::new(Srv::No)) {
            Err(o) => o,
            Ok(shader) => sweep(&shader, &[View::RawUav(4)], &[], &cases, |c, got| {
                let want = [if c[0] == c[1] { c[2] } else { c[0] }, c[0]];
                (got[0][..2] != want).then(|| format!("mem {:#x} cmp {:#x}: got {:x?} want {want:x?}", c[0], c[1], &got[0][..2]))
            }),
        }
    };
    out.push(("buffer_atomic_cmpswap".to_string(), cs));
    out
}

// ---- the tests ----

fn rows_test(family: &str, rows: impl FnOnce() -> Vec<(String, Outcome)>) {
    if Host::new(false).is_err() {
        eprintln!("{family}: no device; not run");
        return;
    }
    report_family(family, rows());
}

#[test]
fn oracle_ds_read_write() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("lds_rw", lds_sequential);
}

#[test]
fn oracle_ds_atomics() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("lds_atomic", lds_atomic_rows);
}

#[test]
fn oracle_ds_cross_lane() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("lds_cross_lane", || {
        let mut v = swizzle_rows();
        v.extend(bpermute_rows());
        v
    });
}

#[test]
fn oracle_ds_bank_patterns() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("lds_bank", bank_patterns);
}

#[test]
fn oracle_buffer_atomics() {
    let _chain = super::oracle_tests::metal_chain();
    rows_test("buffer_atomic", buffer_atomic_rows);
}

#[test]
fn every_ds_row_names_a_gfx10_mnemonic() {
    let _chain = super::oracle_tests::metal_chain();
    for r in seq_rows() {
        assert!(crate::gfx10::opcode_of(r.name).is_some(), "{}", r.name);
    }
}
