// Checks softref.h against the host FPU (native build, any arch): clang -O1 validate-softref.c -o v && ./v
// Needs a host whose fmaf/fma are correctly rounded in all four modes (arm64 and x86 with FMA are).
#include <stdio.h>
#include <string.h>
#include <fenv.h>
#include <math.h>
#include "../softref.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static uint32_t rf(void) { uint32_t x = (uint32_t)rnd(); switch (rnd() & 7) { case 0: x &= 0x807FFFFF; break; case 1: x = (x & 0x807FFFFF) | ((120 + (rnd() & 15)) << 23); break; case 2: x = (x & 0x80000000) | ((uint32_t)(rnd() % 3) ? 0 : 0x7F7FFFFF); break; default: break; } return x; }
int main(void) {
    int modes[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO}, bad = 0; long n = 0;
    for (int rm = 0; rm < 4; rm++) {
        fesetround(modes[rm]);
        for (long i = 0; i < 3000000; i++) {
            uint32_t a = rf(), b = rf(), c = rf(); float fa, fb, fc; memcpy(&fa, &a, 4); memcpy(&fb, &b, 4); memcpy(&fc, &c, 4);
            if (isnan(fa) || isnan(fb) || isnan(fc)) continue;
            if (i % 3 == 0) { volatile float p = fa * fb; fc = -p; if (i % 6 == 0) fc = nextafterf(fc, 1e30f); memcpy(&c, &fc, 4); if (isnan(fc)) continue; }
            float r = fmaf(fa, fb, fc); uint32_t rb; memcpy(&rb, &r, 4);
            uint32_t m = (uint32_t)sf_fma(F32, a, b, c, 0, 0, rm); n++;
            if (isnan(r)) { if (m != 0xFFC00000u) { if (bad++ < 5) printf("f32 nan rm=%d %08x %08x %08x host %08x model %08x\n", rm, a, b, c, rb, m); } continue; }
            if (rb != m && bad++ < 10) printf("f32 rm=%d %08x %08x %08x host %08x model %08x\n", rm, a, b, c, rb, m);
        }
        for (long i = 0; i < 2000000; i++) {
            uint64_t a = rnd(), b = rnd(), c = rnd();
            switch (rnd() & 3) { case 0: a = (a & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(1000 + (rnd() & 47)) << 52); b = (b & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(1000 + (rnd() & 47)) << 52); break; case 1: a &= 0x800FFFFFFFFFFFFFull; break; default: break; }
            double da, db, dc; memcpy(&da, &a, 8); memcpy(&db, &b, 8); memcpy(&dc, &c, 8);
            if (isnan(da) || isnan(db) || isnan(dc)) continue;
            if (i % 3 == 0) { volatile double p = da * db; dc = -p; if (i % 6 == 0) dc = nextafter(dc, 1e300); memcpy(&c, &dc, 8); if (isnan(dc)) continue; }
            double r = fma(da, db, dc); uint64_t rb; memcpy(&rb, &r, 8);
            uint64_t m = sf_fma(F64, a, b, c, 0, 0, rm); n++;
            if (isnan(r)) { continue; }
            if (rb != m && bad++ < 10) printf("f64 rm=%d %016llx %016llx %016llx host %016llx model %016llx\n", rm, (unsigned long long)a, (unsigned long long)b, (unsigned long long)c, (unsigned long long)rb, (unsigned long long)m);
        }
        // float -> half (arm64 host converts with the current rounding mode: __fp16 cast); half -> float exact
#if defined(__aarch64__)
        for (long i = 0; i < 4000000; i++) {
            uint32_t x = rf(); if (i % 2) { uint16_t h = (uint16_t)rnd(); x = sf_h2f(h) + (uint32_t)((int)(rnd() % 0x4001) - 0x2000); }
            float fx; memcpy(&fx, &x, 4); if (isnan(fx)) continue;
            __fp16 hh = (__fp16)fx; uint16_t hb; memcpy(&hb, &hh, 2);
            uint16_t m = sf_f2h(x, rm); n++;
            if (hb != m && bad++ < 20) printf("f2h rm=%d %08x host %04x model %04x\n", rm, x, hb, m);
        }
#endif
    }
    for (uint32_t h = 0; h < 65536; h++) {
        uint32_t m = sf_h2f((uint16_t)h);
#if defined(__aarch64__)
        __fp16 hh; uint16_t hb = (uint16_t)h; memcpy(&hh, &hb, 2); float f = (float)hh; uint32_t fb; memcpy(&fb, &f, 4);
        if (!sf_isnan(F16, h) && fb != m && bad++ < 20) printf("h2f %04x host %08x model %08x\n", h, fb, m);
#endif
        (void)m;
    }
    printf("softref: %ld cases checked, %d mismatches\n", n, bad);
    return bad != 0;
}
