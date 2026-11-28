// Reproducer of the vpsllvq/vpsrlvq gap (Rosetta uses only the low 32 bits of each 64-bit count lane). Build as a Windows exe, run through tools/mac/rosetta-probe-wine.sh style launch; every line whose result differs from "intel" is the gap.
#include <stdio.h>
#include <stdint.h>
static uint64_t shl(uint64_t a, uint64_t c) { uint64_t r[2], x[2] = {a, a}, y[2] = {c, c}; __asm__ volatile("vmovdqu (%1),%%xmm1\n vmovdqu (%2),%%xmm2\n vpsllvq %%xmm2,%%xmm1,%%xmm0\n vmovdqu %%xmm0,(%0)" :: "r"(r), "r"(x), "r"(y) : "xmm0", "xmm1", "xmm2", "memory"); return r[0]; }
static uint64_t shr(uint64_t a, uint64_t c) { uint64_t r[2], x[2] = {a, a}, y[2] = {c, c}; __asm__ volatile("vmovdqu (%1),%%xmm1\n vmovdqu (%2),%%xmm2\n vpsrlvq %%xmm2,%%xmm1,%%xmm0\n vmovdqu %%xmm0,(%0)" :: "r"(r), "r"(x), "r"(y) : "xmm0", "xmm1", "xmm2", "memory"); return r[0]; }
static uint32_t shl32(uint32_t a, uint32_t c) { uint32_t r[4], x[4] = {a, a, a, a}, y[4] = {c, c, c, c}; __asm__ volatile("vmovdqu (%1),%%xmm1\n vmovdqu (%2),%%xmm2\n vpsllvd %%xmm2,%%xmm1,%%xmm0\n vmovdqu %%xmm0,(%0)" :: "r"(r), "r"(x), "r"(y) : "xmm0", "xmm1", "xmm2", "memory"); return r[0]; }
static uint32_t sar32(uint32_t a, uint32_t c) { uint32_t r[4], x[4] = {a, a, a, a}, y[4] = {c, c, c, c}; __asm__ volatile("vmovdqu (%1),%%xmm1\n vmovdqu (%2),%%xmm2\n vpsravd %%xmm2,%%xmm1,%%xmm0\n vmovdqu %%xmm0,(%0)" :: "r"(r), "r"(x), "r"(y) : "xmm0", "xmm1", "xmm2", "memory"); return r[0]; }
int main(void) {
    uint64_t cs[] = {0, 1, 63, 64, 65, 127, 128, 129, 191, 192, 255, 256, 257, 319, 320, 0x1000, 0x10001, 0x100000000ull, 0x100000001ull, 0x100000040ull, 0x8000000000000000ull, 0x8000000000000001ull, 0xFFFFFFFFFFFFFFFFull};
    for (unsigned i = 0; i < sizeof cs / 8; i++) printf("count=%016llx  vpsllvq(0x8000000000000001)=%016llx intel=%016llx | vpsrlvq=%016llx intel=%016llx\n", (unsigned long long)cs[i], (unsigned long long)shl(0x8000000000000001ull, cs[i]), (unsigned long long)(cs[i] > 63 ? 0 : 0x8000000000000001ull << cs[i]), (unsigned long long)shr(0x8000000000000001ull, cs[i]), (unsigned long long)(cs[i] > 63 ? 0 : 0x8000000000000001ull >> cs[i]));
    uint32_t c3[] = {31, 32, 33, 255, 256, 257, 0x10000, 0x80000000u, 0xFFFFFFFFu};
    for (unsigned i = 0; i < sizeof c3 / 4; i++) printf("count=%08x vpsllvd(0x80000001)=%08x intel=%08x | vpsravd=%08x intel=%08x\n", c3[i], shl32(0x80000001u, c3[i]), c3[i] > 31 ? 0 : 0x80000001u << c3[i], sar32(0x80000001u, c3[i]), c3[i] > 31 ? 0xFFFFFFFFu : (uint32_t)((int32_t)0x80000001u >> c3[i]));
    return 0;
}
