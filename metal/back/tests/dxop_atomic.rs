//! 32-bit atomics of the DXIL -> MSC hop: `atomicBinOp.i32` (1,638 programs:
//! 1,016 pixel, 622 compute) and `atomicCompareExchange.i32` (26 compute),
//! on a root UAV byte buffer (`InterlockedX` on a `RWByteAddressBuffer`) and
//! on group shared memory. Atomics are checked by what no ordering can
//! change: the totals, and that the returned original values are a
//! permutation of the intermediate states.
//!
//! Spec: D3D11.3 Functional Spec 7.14.x "Atomic operations" (Add, And, Or,
//! Xor, Min/Max signed and unsigned, Exchange, CompareExchange: the original
//! value is returned); helper lanes perform no UAV atomics (the same spec,
//! "UAV writes by helper pixels are discarded").
#![cfg(target_vendor = "apple")]

mod common_dxop;
mod common_msc;

use common_dxop::*;

/// Every thread of 8 groups of 64 applies each operation once, to its own
/// word per operation; the totals are what the CPU gets by applying the same
/// operands in any order (all of these operations commute).
#[test]
fn atomic_binops_on_a_uav_commute_to_the_cpu_total() {
    let rig = rig();
    let Some(p) = rig.program(
        "atomic_bin",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           uint v = I.Load(t.x*4);\n\
           uint o;\n\
           O.InterlockedAdd(0, v, o);\n\
           O.InterlockedAnd(4, v | 0xF0000000u, o);\n\
           O.InterlockedOr(8, v, o);\n\
           O.InterlockedXor(12, v, o);\n\
           O.InterlockedMin(16, v, o);\n\
           O.InterlockedMax(20, v, o);\n\
           int s = (int)v;\n\
           O.InterlockedMin(24, s, o);\n\
           O.InterlockedMax(28, s, o);\n\
           O.InterlockedAdd(32, 1, o);\n\
           O.Store(1024 + t.x*4, o); }\n",
        &["dx.op.atomicBinOp.i32"],
    ) else {
        return;
    };
    let n = 512usize;
    let mut s = 12345u32;
    let input: Vec<u32> = (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            s
        })
        .collect();
    let mut init = vec![0u32; 8];
    let _ = &mut init;
    let got = rig.run(&p, [8, 1, 1], [64, 1, 1], &[], &input, 256 + n);
    // O starts as 0xDEADBEEF everywhere; account for it as the initial value.
    let init = 0xDEAD_BEEFu32;
    let add = input.iter().fold(init, |a, &b| a.wrapping_add(b));
    let and = input.iter().fold(init, |a, &b| a & (b | 0xF000_0000));
    let or = input.iter().fold(init, |a, &b| a | b);
    let xor = input.iter().fold(init, |a, &b| a ^ b);
    let umin = input.iter().fold(init, |a, &b| a.min(b));
    let umax = input.iter().fold(init, |a, &b| a.max(b));
    let smin = input.iter().fold(init as i32, |a, &b| a.min(b as i32)) as u32;
    let smax = input.iter().fold(init as i32, |a, &b| a.max(b as i32)) as u32;
    assert_eq!(
        &got[0..8],
        &[add, and, or, xor, umin, umax, smin, smax],
        "add and or xor umin umax smin smax"
    );
    // The counter at 32 started at 0xDEADBEEF: the originals the threads saw are n distinct values in a row.
    assert_eq!(got[8], init.wrapping_add(n as u32));
    let mut seen: Vec<u32> = got[256..256 + n].to_vec();
    seen.sort_unstable();
    let want: Vec<u32> = (0..n as u32).map(|i| init.wrapping_add(i)).collect();
    let mut want_sorted = want.clone();
    want_sorted.sort_unstable();
    assert_eq!(
        seen, want_sorted,
        "the originals returned by InterlockedAdd(1) are distinct, consecutive"
    );
}

/// `InterlockedExchange` and `InterlockedCompareExchange`, one thread:
/// the original comes back, memory changes only when the comparison matches.
#[test]
fn compare_exchange_returns_the_original_and_only_writes_on_a_match() {
    let rig = rig();
    let Some(p) = rig.program(
        "atomic_cas_single",
        "[numthreads(1,1,1)] void main() {\n\
           uint o;\n\
           O.Store(0, 5);\n\
           O.InterlockedCompareExchange(0, 5, 9, o); O.Store(16, o); O.Store(20, O.Load(0));\n\
           O.InterlockedCompareExchange(0, 5, 11, o); O.Store(24, o); O.Store(28, O.Load(0));\n\
           O.InterlockedExchange(0, 77, o); O.Store(32, o); O.Store(36, O.Load(0));\n\
           O.Store(4, 0xFFFFFFFFu);\n\
           O.InterlockedCompareExchange(4, 0xFFFFFFFFu, 3, o); O.Store(40, o); O.Store(44, O.Load(4));\n\
           O.Store(8, 0x80000000u);\n\
           O.InterlockedCompareExchange(8, 0x7FFFFFFFu, 1, o); O.Store(48, o); O.Store(52, O.Load(8)); }\n",
        &["dx.op.atomicCompareExchange.i32"],
    ) else {
        return;
    };
    let got = rig.run(&p, [1, 1, 1], [1, 1, 1], &[], &[], 16);
    // original / memory after: match 5->9; mismatch (9 != 5); exchange 77 returns 9; CAS on all-ones; no match
    assert_eq!(
        &got[4..14],
        &[5, 9, 9, 9, 9, 77, 0xFFFF_FFFF, 3, 0x8000_0000, 0x8000_0000]
    );
}

/// A compare-exchange loop makes an increment; 512 threads do it on one word.
/// The word ends `start + 512` and the values the threads saw at their
/// successful exchange are 512 distinct ones (`start` .. `start + 511`).
#[test]
fn compare_exchange_loop_increments_exactly_once_per_thread() {
    let rig = rig();
    let Some(p) = rig.program(
        "atomic_cas_loop",
        "[numthreads(64,1,1)] void main(uint3 t : SV_DispatchThreadID) {\n\
           uint cur = k0, seen = 0;\n\
           uint tries = 0;\n\
           [loop] while (tries < 200000) {\n\
             O.InterlockedCompareExchange(0, cur, cur + 1, seen);\n\
             if (seen == cur) break;\n\
             cur = seen; tries++;\n\
           }\n\
           O.Store(1024 + t.x*4, tries < 200000 ? cur : 0xFFFFFFFFu); }\n",
        &["dx.op.atomicCompareExchange.i32"],
    ) else {
        return;
    };
    let start = 0xDEAD_BEEFu32; // what the rig fills O with
    let n = 512usize;
    let got = rig.run(&p, [8, 1, 1], [64, 1, 1], &[start], &[], 256 + n);
    assert_eq!(got[0], start.wrapping_add(n as u32), "512 increments");
    let mut seen: Vec<u32> = got[256..256 + n].to_vec();
    seen.sort_unstable();
    let want: Vec<u32> = (0..n as u32).map(|i| start.wrapping_add(i)).collect();
    let mut want = want;
    want.sort_unstable();
    assert_eq!(seen, want);
}

/// Atomics on group shared memory (`atomicrmw` / `cmpxchg` of LLVM, not a
/// `dx.op`): the totals of a group, and the same through the UAV.
#[test]
fn group_shared_atomics_total_up() {
    let rig = rig();
    let Some(p) = rig.program(
        "atomic_lds",
        "groupshared uint acc[3];\n\
         [numthreads(128,1,1)] void main(uint gi : SV_GroupIndex, uint3 g : SV_GroupID) {\n\
           if (gi < 3) acc[gi] = 0;\n\
           GroupMemoryBarrierWithGroupSync();\n\
           uint o;\n\
           InterlockedAdd(acc[0], gi + 1, o);\n\
           InterlockedMax(acc[1], gi * 3, o);\n\
           InterlockedOr(acc[2], 1u << (gi & 31), o);\n\
           GroupMemoryBarrierWithGroupSync();\n\
           if (gi == 0) O.Store3(g.x * 12, uint3(acc[0], acc[1], acc[2])); }\n",
        &["dx.op.barrier"],
    ) else {
        return;
    };
    let got = rig.run(&p, [6, 1, 1], [128, 1, 1], &[], &[], 18);
    for g in 0..6 {
        assert_eq!(
            &got[g * 3..g * 3 + 3],
            &[128 * 129 / 2, 127 * 3, 0xFFFF_FFFF],
            "group {g}"
        );
    }
}

/// In a pixel shader: the uncovered pixels of a quad are helper lanes and do
/// not perform UAV atomics, so the counter equals the number of covered
/// pixels (the lower-left half of an 8x8 target: 36), and every covered
/// pixel got its own ticket.
#[test]
fn pixel_shader_atomics_skip_helper_lanes() {
    let rig = rig();
    let Some(p) = rig.pixel_program_vs(
        "atomic_px",
        HALF_VS,
        "void main(float4 pos : SV_Position) {\n\
           uint x = (uint)pos.x, y = (uint)pos.y;\n\
           uint t; O.InterlockedAdd(0, 1, t);\n\
           O.Store(64 + (y*8+x)*4, t); }\n",
        &["dx.op.atomicBinOp.i32"],
        false,
    ) else {
        return;
    };
    let start = 0xDEAD_BEEFu32;
    let got = rig.run_pixels(&p, [8, 8], &[], &[], 16 + 64, None);
    assert_eq!(
        got[0],
        start.wrapping_add(36),
        "one atomic per covered pixel"
    );
    let mut tickets = Vec::new();
    for j in 0..8usize {
        for i in 0..8usize {
            let w = got[16 + j * 8 + i];
            if i <= j {
                tickets.push(w);
            } else {
                assert_eq!(w, 0xDEAD_BEEF, "uncovered pixel ({i},{j}) stored");
            }
        }
    }
    tickets.sort_unstable();
    assert_eq!(
        tickets,
        (0..36u32)
            .map(|k| start.wrapping_add(k))
            .collect::<Vec<_>>()
    );
}
