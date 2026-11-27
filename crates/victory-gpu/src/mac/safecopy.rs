//! Safe copies without the AVX masked store (Mac patch G4, see
//! docs/mac/PATCHES.md).
//!
//! `crate::safecopy` stores the dwords that changed with `vpmaskmovd` (in
//! `victory_safe_put` and `victory_safe_merge`) and relies on a fault on a
//! page that is not there, or is guarded, being taken by the handler of
//! `crate::watch` and sent to the failing exit. Rosetta 2 cannot take a fault
//! of an AVX masked store: `rosetta error: unexpectedly need to EmulateForward
//! on a synchronous exception`, and the process stops for good (VIctory-macOS
//! issue #1; the loader's `mac/masked.rs` rewrites the title's own such
//! stores for the same reason, but it cannot reach ours).
//!
//! Here the same two routines store each changed dword with a plain 32-bit
//! `mov`, which Rosetta resumes or abandons like any store. The reads of
//! title memory stay 256-bit loads (a load that faults changes nothing).
//! Same arguments, same results, same failing exits (`failing_exit`), but for
//! the hooks in `crate::safecopy`: `put_changed` and `merge` call [`put`] and
//! [`merge`] here, and `failing_exit` asks [`failing_exit`] too.
//!
//! In use under Rosetta, or anywhere with `VICTORY_SAFE_SCALAR=1`
//! (`=0`: never; it lets a PC and the tests cover this code).

use std::sync::LazyLock;

std::arch::global_asm!(
    ".globl victory_mac_put",
    ".globl victory_mac_put_at",
    ".globl victory_mac_put_after",
    ".globl victory_mac_put_failed",
    ".globl victory_mac_merge",
    ".globl victory_mac_merge_at",
    ".globl victory_mac_merge_after",
    ".globl victory_mac_merge_failed",
    // As `victory_safe_put`: rcx destination, rdx what was there, r8 what is
    // to be there, r9 blocks of eight dwords. Each dword that differs is one
    // `mov`; the registers are the volatile ones (r10 carries the dword).
    "victory_mac_put:",
    "    test r9, r9",
    "    jz 4f",
    "2:",
    "    vmovdqu ymm1, ymmword ptr [r8]",
    "    vpcmpeqd ymm2, ymm1, ymmword ptr [rdx]",
    "    vmovmskps eax, ymm2",
    "    xor eax, 255",
    "    jz 3f",
    "victory_mac_put_at:",
    "    test al, 1",
    "    jz 20f",
    "    mov r10d, dword ptr [r8 + 0]",
    "    mov dword ptr [rcx + 0], r10d",
    "20:",
    "    test al, 2",
    "    jz 21f",
    "    mov r10d, dword ptr [r8 + 4]",
    "    mov dword ptr [rcx + 4], r10d",
    "21:",
    "    test al, 4",
    "    jz 22f",
    "    mov r10d, dword ptr [r8 + 8]",
    "    mov dword ptr [rcx + 8], r10d",
    "22:",
    "    test al, 8",
    "    jz 23f",
    "    mov r10d, dword ptr [r8 + 12]",
    "    mov dword ptr [rcx + 12], r10d",
    "23:",
    "    test al, 16",
    "    jz 24f",
    "    mov r10d, dword ptr [r8 + 16]",
    "    mov dword ptr [rcx + 16], r10d",
    "24:",
    "    test al, 32",
    "    jz 25f",
    "    mov r10d, dword ptr [r8 + 20]",
    "    mov dword ptr [rcx + 20], r10d",
    "25:",
    "    test al, 64",
    "    jz 26f",
    "    mov r10d, dword ptr [r8 + 24]",
    "    mov dword ptr [rcx + 24], r10d",
    "26:",
    "    test al, 128",
    "    jz 27f",
    "    mov r10d, dword ptr [r8 + 28]",
    "    mov dword ptr [rcx + 28], r10d",
    "27:",
    "victory_mac_put_after:",
    "    nop",
    "3:",
    "    add rcx, 32",
    "    add rdx, 32",
    "    add r8, 32",
    "    dec r9",
    "    jnz 2b",
    "    vzeroupper",
    "4:",
    "    mov eax, 1",
    "    ret",
    "victory_mac_put_failed:",
    "    vzeroupper",
    "    xor eax, eax",
    "    ret",
    // As `victory_safe_merge`: rcx title memory, rdx what the card holds, r8
    // what it held, r9 blocks of 32 bytes, fifth the two counts; rax the
    // blocks left. All volatile registers are in use, so rbx (kept on the
    // stack for the call, and given back by both exits) carries the dwords,
    // which are taken out of the blend in ymm3/ymm4.
    "victory_mac_merge:",
    "    push rbx",
    "    xor r10d, r10d",
    "    xor r11d, r11d",
    "    test r9, r9",
    "    jz 6f",
    "2:",
    "    vmovdqu ymm0, ymmword ptr [rdx]",
    "    vpcmpeqb ymm1, ymm0, ymmword ptr [r8]",
    "    vpmovmskb eax, ymm1",
    "    cmp eax, -1",
    "    je 5f",
    "victory_mac_merge_at:",
    "    vmovdqu ymm2, ymmword ptr [rcx]",
    "    vpblendvb ymm3, ymm0, ymm2, ymm1",
    "    vpcmpeqd ymm4, ymm3, ymm2",
    "    vmovmskps eax, ymm4",
    "    xor eax, 255",
    "    jz 4f",
    "    vextracti128 xmm4, ymm3, 1",
    "    test al, 1",
    "    jz 20f",
    "    vmovd ebx, xmm3",
    "    mov dword ptr [rcx + 0], ebx",
    "20:",
    "    test al, 2",
    "    jz 21f",
    "    vpextrd ebx, xmm3, 1",
    "    mov dword ptr [rcx + 4], ebx",
    "21:",
    "    test al, 4",
    "    jz 22f",
    "    vpextrd ebx, xmm3, 2",
    "    mov dword ptr [rcx + 8], ebx",
    "22:",
    "    test al, 8",
    "    jz 23f",
    "    vpextrd ebx, xmm3, 3",
    "    mov dword ptr [rcx + 12], ebx",
    "23:",
    "    test al, 16",
    "    jz 24f",
    "    vmovd ebx, xmm4",
    "    mov dword ptr [rcx + 16], ebx",
    "24:",
    "    test al, 32",
    "    jz 25f",
    "    vpextrd ebx, xmm4, 1",
    "    mov dword ptr [rcx + 20], ebx",
    "25:",
    "    test al, 64",
    "    jz 26f",
    "    vpextrd ebx, xmm4, 2",
    "    mov dword ptr [rcx + 24], ebx",
    "26:",
    "    test al, 128",
    "    jz 27f",
    "    vpextrd ebx, xmm4, 3",
    "    mov dword ptr [rcx + 28], ebx",
    "27:",
    "victory_mac_merge_after:",
    "    popcnt ebx, eax",
    "    add r11, rbx",
    "4:",
    "    inc r10",
    "    vmovdqu ymmword ptr [r8], ymm0",
    "5:",
    "    add rcx, 32",
    "    add rdx, 32",
    "    add r8, 32",
    "    dec r9",
    "    jnz 2b",
    "6:",
    "victory_mac_merge_failed:",
    "    vzeroupper",
    "    mov rax, qword ptr [rsp + 48]",
    "    mov qword ptr [rax], r10",
    "    mov qword ptr [rax + 8], r11",
    "    mov rax, r9",
    "    pop rbx",
    "    ret",
);

unsafe extern "system" {
    fn victory_mac_put(dst: *mut u8, was: *const u8, now: *const u8, blocks: usize) -> u32;
    fn victory_mac_put_at();
    fn victory_mac_put_after();
    fn victory_mac_put_failed();
    fn victory_mac_merge(title: *mut u8, now: *const u8, held: *mut u8, blocks: usize, counts: *mut [u64; 2]) -> usize;
    fn victory_mac_merge_at();
    fn victory_mac_merge_after();
    fn victory_mac_merge_failed();
}

/// Whether the stores of `crate::safecopy` are the scalar ones of this module.
pub(crate) fn scalar() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| match std::env::var("VICTORY_SAFE_SCALAR").ok().as_deref() {
        Some("1") => true,
        Some("0") => false,
        _ => super::rosetta::rosetta(),
    });
    *ON
}

/// `victory_safe_put`, or its scalar twin where [`scalar`].
///
/// # Safety
/// `crate::safecopy::victory_safe_put`'s.
pub(crate) unsafe fn put(dst: *mut u8, was: *const u8, now: *const u8, blocks: usize, plain: unsafe extern "system" fn(*mut u8, *const u8, *const u8, usize) -> u32) -> u32 {
    // SAFETY: the caller's.
    unsafe { if scalar() { victory_mac_put(dst, was, now, blocks) } else { plain(dst, was, now, blocks) } }
}

/// `victory_safe_merge`, or its scalar twin where [`scalar`].
///
/// # Safety
/// `crate::safecopy::victory_safe_merge`'s.
pub(crate) unsafe fn merge(
    title: *mut u8,
    now: *const u8,
    held: *mut u8,
    blocks: usize,
    counts: *mut [u64; 2],
    plain: unsafe extern "system" fn(*mut u8, *const u8, *mut u8, usize, *mut [u64; 2]) -> usize,
) -> usize {
    // SAFETY: the caller's.
    unsafe { if scalar() { victory_mac_merge(title, now, held, blocks, counts) } else { plain(title, now, held, blocks, counts) } }
}

/// For `crate::safecopy::failing_exit`: where a fault at `at` in one of the
/// scalar routines goes on.
pub(crate) fn failing_exit(at: usize) -> Option<usize> {
    let within = |from: unsafe extern "system" fn(), to: unsafe extern "system" fn()| (from as usize..to as usize).contains(&at);
    if within(victory_mac_put_at, victory_mac_put_after) {
        return Some(victory_mac_put_failed as *const () as usize);
    }
    within(victory_mac_merge_at, victory_mac_merge_after).then_some(victory_mac_merge_failed as *const () as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scalar routines do what the AVX ones do, block by block, on
    /// memory that is there: the same bytes in title memory and in what the
    /// card held, the same counts.
    #[test]
    fn the_scalar_routines_equal_the_masked_ones() {
        unsafe extern "system" {
            fn victory_safe_put(dst: *mut u8, was: *const u8, now: *const u8, blocks: usize) -> u32;
            fn victory_safe_merge(title: *mut u8, now: *const u8, held: *mut u8, blocks: usize, counts: *mut [u64; 2]) -> usize;
        }
        let mut seed = 7u32;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        const N: usize = 9 * 32;
        for round in 0..2000 {
            let blocks = 1 + random() as usize % 9;
            let len = blocks * 32;
            let (mut a, mut b): ([u8; N], [u8; N]) = ([0; N], [0; N]);
            let was: Vec<u8> = (0..len).map(|_| random() as u8).collect();
            let mut now = was.clone();
            for _ in 0..random() % 20 {
                let i = random() as usize % len;
                now[i] = if round % 2 == 0 { random() as u8 } else { now[i].wrapping_add(1) };
            }
            if round % 5 == 0 {
                now.iter_mut().for_each(|x| *x = x.wrapping_add(3));
            }
            let title: Vec<u8> = (0..len).map(|_| random() as u8).collect();
            a[..len].copy_from_slice(&title);
            b[..len].copy_from_slice(&title);
            // SAFETY: arrays of ours, `len` bytes each.
            unsafe {
                assert_eq!(victory_safe_put(a.as_mut_ptr(), was.as_ptr(), now.as_ptr(), blocks), 1);
                assert_eq!(victory_mac_put(b.as_mut_ptr(), was.as_ptr(), now.as_ptr(), blocks), 1);
            }
            assert_eq!(a, b, "put, round {round}");
            a[..len].copy_from_slice(&title);
            b[..len].copy_from_slice(&title);
            let (mut held_a, mut held_b) = (was.clone(), was.clone());
            let (mut counts_a, mut counts_b) = ([0u64; 2], [0u64; 2]);
            // SAFETY: as above.
            unsafe {
                assert_eq!(victory_safe_merge(a.as_mut_ptr(), now.as_ptr(), held_a.as_mut_ptr(), blocks, &mut counts_a), 0);
                assert_eq!(victory_mac_merge(b.as_mut_ptr(), now.as_ptr(), held_b.as_mut_ptr(), blocks, &mut counts_b), 0);
            }
            assert_eq!((a, held_a, counts_a), (b, held_b, counts_b), "merge, round {round}");
        }
    }
}
