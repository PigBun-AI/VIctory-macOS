//! C-V-001 (docs/mac/PATCHES.md G9, G12): the console device's two pipeline-cache methods, slots 0x1c8 and 0x218.
//! The title (title+0x2477578, title+0x24800d5) treats a failed call as a cache miss and makes the pipeline from
//! its containers through 0x2b0 / 0x1f0, so what matters is that a miss is a clean failure: `E_NOTIMPL`, `out` set to
//! null (the title keeps whatever `out` names on success), nothing read through the record's pointers.
//!
//! Reference: the record words of the run 20261127-221938-map (`research/game-runs/.../stderr.log`, 16 qwords per
//! call, the first calls of each slot) and the title's disassembly as decoded in G9.
use crate::object::{E_NOTIMPL, HRESULT, Iid, This, slot};

type Method = extern "system" fn(This, *const u64, *const Iid, *mut This) -> HRESULT;

/// The first calls of the run, as logged.
const GRAPHICS: [[u64; 16]; 2] = [
    [0x16f49820, 0x14934e024, 0x184, 0x14934e1a8, 0xbc, 0xe18027300, 0x200, 0x14934e264, 0xd0, 0xe18027500, 0x200, 0x14934e334, 0xb0, 0, 0, 0],
    [0x4518a230, 0x14aca31f0, 0x200, 0x14aca33f0, 0x50, 0xe24640500, 0x4a40, 0x14abe8634, 0x5b0, 0, 0, 0, 0, 0xe24645000, 0xd80, 0x14abe8be4],
];
const COMPUTE: [[u64; 16]; 2] = [
    [0x16d1dbd0, 0x14934a0f0, 0x1c, 0x14934a10c, 0x14, 0xe18020000, 0x180, 0x14934a120, 0x118, 0x14bad26e0, 0x14baa1de0, 0xd7e7b1a22f0c3802, 0, 0x14934a0f0, 0x1c, 0x14934a238],
    [0x1fc3d350, 0x149570bc4, 0x1c, 0x149a48e78, 0x14, 0xe1cdcfb00, 0x1f9c0, 0x149a48e8c, 0x1798, 0x14bac7e80, 0x14bac7d60, 0xc6a0f3729124f10a, 0, 0x149572a94, 0x1c, 0x149a4a624],
];

/// With the record's pointers dangling (they point into the title's memory, which these tests do not have), a call
/// fails cleanly and leaves `out` null: the title then goes to 0x2b0 and 0x1f0. (`VICTORY_MAC_PSO_X` is off in tests.)
#[test]
fn a_cache_miss_is_a_clean_failure() {
    assert!(std::env::var_os("VICTORY_MAC_PSO_X").is_none_or(|v| v != "1"), "this test is for the default (off) behaviour");
    let device = crate::device::device();
    for (offset, records) in [(0x1c8usize, &GRAPHICS), (0x218, &COMPUTE)] {
        let call = slot::<Method>(device, offset);
        for record in records {
            let mut out: This = 0x1234 as This;
            assert_eq!(call(device, record.as_ptr(), std::ptr::null(), &mut out), E_NOTIMPL, "slot {offset:#x}");
            assert!(out.is_null(), "slot {offset:#x}: out is set to null (a stale value would be taken for a pipeline)");
        }
        // a null out pointer (the title passes a local, but a probe may not) is tolerated
        let mut dummy = [0u64; 16];
        dummy[0] = 1;
        assert_eq!(call(device, dummy.as_ptr(), std::ptr::null(), std::ptr::null_mut()), E_NOTIMPL);
    }
}
