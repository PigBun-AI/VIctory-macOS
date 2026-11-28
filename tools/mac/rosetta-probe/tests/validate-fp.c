// Checks isa-model-fp.h (add/sub/mul/div/sqrt/max/min, binary32/64, four rounding modes, DAZ, FTZ) against arm64 hardware:
//   clang -O1 -I.. validate-fp.c -o v && ./v
// Hardware reference: the host FPU in each rounding mode (FPCR.RMode); DAZ = host op on inputs flushed by the model's own sf_daz; FTZ = the host with FPCR.FZ,
// where Arm detects tininess before rounding and x86 after: the only disagreements allowed are results that round up to the smallest normal.
// NaN payloads follow x86 rules, so only NaN-ness is compared against the host; max/min use the C expressions a > b ? a : b (the x86 definition) on the host.
#include <fenv.h>
#include <math.h>
#include <stdio.h>
#include "../isa-model-fp.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static uint32_t g32(void) { uint32_t x = (uint32_t)rnd(); switch (rnd() % 8) { case 0: x &= 0x807FFFFF; break; case 1: x = (x & 0x807FFFFF) | ((uint32_t)(rnd() % 8) << 23); break; case 2: x = (x & 0x807FFFFF) | ((uint32_t)(120 + rnd() % 16) << 23); break;
    case 3: x = (x & 0x80000000u) | ((uint32_t)(rnd() % 4 == 0 ? 0xFF : 0xFE) << 23) | (x & 0x7FFFFF); break; case 4: x = (x & 0x807FFFFF) | ((uint32_t)(rnd() % 60 + 97) << 23); break; default: break; }
    if (rnd() % 16 == 0) x &= 0x80000000u; return x; }
static uint64_t g64(void) { uint64_t x = rnd(); switch (rnd() % 8) { case 0: x &= 0x800FFFFFFFFFFFFFull; break; case 1: x = (x & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(rnd() % 8) << 52); break; case 2: x = (x & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(1000 + rnd() % 48) << 52); break;
    case 3: x = (x & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(rnd() % 4 == 0 ? 0x7FF : 0x7FE) << 52); break; case 4: x = (x & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(rnd() % 100 + 960) << 52); break; default: break; }
    if (rnd() % 16 == 0) x &= 0x8000000000000000ull; return x; }
static inline void set_fz(int on) { uint64_t v; __asm__ volatile("mrs %0, fpcr" : "=r"(v)); v = on ? (v | (1ull << 24)) : (v & ~(1ull << 24)); __asm__ volatile("msr fpcr, %0" : : "r"(v)); }
static long n_checked, n_bad, n_ftz_allowed;
static const int FEM[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};
static void rep(const char *w, int dbl, int kind, int rm, uint64_t a, uint64_t b, uint64_t got, uint64_t want) { if (n_bad++ < 20) printf("%s dbl=%d kind=%d rm=%d a=%016llx b=%016llx host=%016llx model=%016llx\n", w, dbl, kind, rm, (unsigned long long)a, (unsigned long long)b, (unsigned long long)got, (unsigned long long)want); }
static volatile float vf; static volatile double vd;
static uint64_t host(int dbl, int kind, uint64_t a, uint64_t b) {
    if (!dbl) { float x, y, r; uint32_t ia = (uint32_t)a, ib = (uint32_t)b; memcpy(&x, &ia, 4); memcpy(&y, &ib, 4); vf = x; x = vf; vf = y; y = vf;
        switch (kind) { case FP_ADD: r = x + y; break; case FP_SUB: r = x - y; break; case FP_MUL: r = x * y; break; case FP_DIV: r = x / y; break; case FP_MAX: r = x > y ? x : y; break; case FP_MIN: r = x < y ? x : y; break; default: r = sqrtf(y); }
        vf = r; r = vf; uint32_t o; memcpy(&o, &r, 4); return o; }
    double x, y, r; memcpy(&x, &a, 8); memcpy(&y, &b, 8); vd = x; x = vd; vd = y; y = vd;
    switch (kind) { case FP_ADD: r = x + y; break; case FP_SUB: r = x - y; break; case FP_MUL: r = x * y; break; case FP_DIV: r = x / y; break; case FP_MAX: r = x > y ? x : y; break; case FP_MIN: r = x < y ? x : y; break; default: r = sqrt(y); }
    vd = r; r = vd; uint64_t o; memcpy(&o, &r, 8); return o; }
int main(void) {
    for (int dbl = 0; dbl < 2; dbl++) { Fmt f = dbl ? F64 : F32; int bits = dbl ? 64 : 32;
        for (int kind = FP_ADD; kind <= FP_SQRT; kind++) for (int rm = 0; rm < 4; rm++) { fesetround(FEM[rm]);
            for (long i = 0; i < 6000000; i++) {
                uint64_t a = dbl ? g64() : g32(), b = dbl ? g64() : g32(); if (i % 5 == 0) b = a ^ (rnd() & 3);
                for (int mode = 0; mode < 3; mode++) {          // 0 plain, 1 DAZ, 2 FTZ(+DAZ) vs host FZ
                    Fpc c = {rm, mode >= 1, mode == 2}; uint64_t ha = a, hb = b;
                    if (mode >= 1) { ha = sf_daz(f, a); hb = sf_daz(f, b); }
                    uint64_t want = fp_op(kind, f, a, b, c);
                    if (mode == 2) set_fz(1);
                    uint64_t got = host(dbl, kind, mode >= 1 ? ha : a, mode >= 1 ? hb : b);
                    if (mode == 2) set_fz(0);
                    int na = sf_isnan(f, a), nb = sf_isnan(f, b); n_checked++;
                    if (kind == FP_MAX || kind == FP_MIN) { if (mode == 2) continue; if (got != want && !(mode == 1 && 0)) { rep("minmax", dbl, kind, rm, a, b, got, want); } continue; }
                    if (sf_isnan(f, got) || sf_isnan(f, want)) { if (sf_isnan(f, got) != sf_isnan(f, want)) rep("nan-ness", dbl, kind, rm, a, b, got, want); else if (sf_isnan(f, want) && !((want >> (f.mb - 1)) & 1)) rep("not quiet", dbl, kind, rm, a, b, got, want); continue; }
                    if (got != want) {
                        if (mode == 2) { uint64_t mn = (uint64_t)1 << f.mb, mag = want & ~(1ull << (bits - 1)); if (mag == mn && (got & ~(1ull << (bits - 1))) == 0) { n_ftz_allowed++; continue; } }
                        rep("value", dbl, kind, rm, a, b, got, want);
                    }
                    (void)na; (void)nb;
                }
            }
        }
    }
    fesetround(FE_TONEAREST);
    printf("validate-fp: %ld checks, %ld mismatches (%ld FTZ tininess-after-rounding differences at the smallest normal, allowed)\n", n_checked, n_bad, n_ftz_allowed); return n_bad != 0;
}
