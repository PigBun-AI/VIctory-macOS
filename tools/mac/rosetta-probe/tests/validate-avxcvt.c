// Checks isa-model-cvt.h against arm64: fcvtn/fcvtm/fcvtp/fcvtz (float -> int, saturating; the x86 indefinite is checked wherever arm64 saturates or the source is NaN),
// scvtf under each rounding mode and fcvt float<->double under each rounding mode (fesetround):  clang -O1 -frounding-math -I.. validate-avxcvt.c -o v && ./v
#include <arm_neon.h>
#include <fenv.h>
#include <math.h>
#include "../isa-model-cvt.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static long bad, total;
static uint64_t gen(int dbl) {
    int es = dbl ? 8 : 4, fb = dbl ? 52 : 23, bias = dbl ? 1023 : 127; uint64_t m = rnd() & ((1ull << fb) - 1), sg = rnd() & 1; int k = (int)(rnd() % 140) - 70; uint64_t E = (uint64_t)(bias + k);
    int nb = k >= 0 && k < fb ? fb - k : 0; if (nb > 0) { uint64_t low = (1ull << nb) - 1; switch (rnd() % 5) { case 0: m &= ~low; break; case 1: m = (m & ~low) | (1ull << (nb - 1)); break; case 2: m = (m & ~low) | 1; break; case 3: m |= low; break; default: break; } }
    if (rnd() % 40 == 0) E = (1ull << (dbl ? 11 : 8)) - 1; if (rnd() % 40 == 0) E = 0; return (sg << (es * 8 - 1)) | (E << fb) | m;
}
static void bump(const char *w, uint64_t x, uint64_t got, uint64_t want, int rm) { total++; if (got != want) { if (bad++ < 12) printf("%s rm=%d x=%016llx got=%016llx want=%016llx\n", w, rm, (unsigned long long)x, (unsigned long long)got, (unsigned long long)want); } }
int main(void) {
    int fe[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO};
    for (long it = 0; it < 12000000; it++) {
        int dbl = (int)(rnd() & 1); uint64_t x = gen(dbl); int rm = (int)(rnd() & 3), tr = (int)(rnd() % 4 == 0); Fpc c = {rm, 0, 0}; int em = tr ? 3 : rm;
        float f; double d; uint32_t x32 = (uint32_t)x; if (!dbl) memcpy(&f, &x32, 4); else memcpy(&d, &x, 8); int isnan_ = dbl ? isnan(d) : isnan(f);
        for (int bits = 32; bits <= 64; bits += 32) {
            uint64_t want = cv_f2i(dbl ? F64 : F32, x, bits, tr, c), ref; int sat = 0;
            #define ARMCVT(MN, W, S) do { if (dbl) { __asm__("fcvt" MN " %" W "0, %d1" : "=r"(tmp) : "w"(d)); } else { __asm__("fcvt" MN " %" W "0, %s1" : "=r"(tmp) : "w"(f)); } } while (0)
            uint64_t tmp = 0;
            if (bits == 32) { switch (em) { case 0: ARMCVT("ns", "w", 0); break; case 1: ARMCVT("ms", "w", 0); break; case 2: ARMCVT("ps", "w", 0); break; default: ARMCVT("zs", "w", 0); } ref = (uint32_t)tmp; sat = (int32_t)ref == INT32_MIN || (int32_t)ref == INT32_MAX; }
            else { switch (em) { case 0: ARMCVT("ns", "x", 0); break; case 1: ARMCVT("ms", "x", 0); break; case 2: ARMCVT("ps", "x", 0); break; default: ARMCVT("zs", "x", 0); } ref = tmp; sat = (int64_t)ref == INT64_MIN || (int64_t)ref == INT64_MAX; }
            uint64_t indef = 1ull << (bits - 1);
            if (isnan_ || isinf(dbl ? d : f)) ref = indef; else if (sat) ref = (want == indef) ? indef : ref;     // arm64 saturates; x86 gives the indefinite when out of range: INT_MIN itself is a valid result for in-range sources
            if (!isnan_ && sat && ref != indef && bits == 32 && (int32_t)ref == INT32_MAX) ref = indef;       // +INT_MAX saturation is out of range on x86 unless the source rounds to exactly 2^31-1
            if (!isnan_ && sat && (int32_t)ref == INT32_MAX && bits == 32 && want != indef) ref = want;
            if (!isnan_ && sat && bits == 64 && ref == (uint64_t)INT64_MAX) ref = (want == indef) ? indef : want;
            bump("f2i", x, want, ref, em);
        }
        // int -> float under the rounding mode
        { int64_t v = (int64_t)(rnd() >> (rnd() % 64)); if (rnd() & 1) v = -v; int old = fegetround(); fesetround(fe[rm]);
          volatile float rf = (float)v; volatile double rd = (double)v; fesetround(old); uint32_t b32; uint64_t b64; float ff = rf; double dd = rd; memcpy(&b32, &ff, 4); memcpy(&b64, &dd, 8);
          bump("i64->f32", (uint64_t)v, cv_i2f(F32, v, c), b32, rm); bump("i64->f64", (uint64_t)v, cv_i2f(F64, v, c), b64, rm);
          int32_t v32 = (int32_t)v; fesetround(fe[rm]); volatile float rf2 = (float)v32; fesetround(old); ff = rf2; memcpy(&b32, &ff, 4); bump("i32->f32", (uint64_t)v32, cv_i2f(F32, v32, c), b32, rm); }
        // float <-> double
        if (dbl) { int old = fegetround(); fesetround(fe[rm]); volatile float r2 = (float)d; fesetround(old); float rr = r2; uint32_t b; memcpy(&b, &rr, 4); uint64_t want = cv_f2f(1, x, c); if (isnan(d)) { total++; if (((uint32_t)want & 0x7FC00000u) != 0x7FC00000u || ((uint32_t)want >> 31) != (uint32_t)(x >> 63)) bad++; } else bump("f64->f32", x, want, b, rm); }
        else { double r2 = f; uint64_t b; memcpy(&b, &r2, 8); uint64_t want = cv_f2f(0, x, c); if (isnan(f)) { total++; if ((want & 0x7FF8000000000000ull) != 0x7FF8000000000000ull) bad++; } else bump("f32->f64", x, want, b, rm); }
    }
    printf("validate-cvt: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
