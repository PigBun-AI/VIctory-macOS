// Checks softref-cvt.h against the host FPU (native build, arm64 or x86): clang -O1 validate-cvt.c -o v && ./v
// Compared where the host agrees with x86: finite in-range float->int (host rounds per the current mode / truncates), int->float and
// float<->double (host rounds per the current mode; NaN skipped). Out-of-range/NaN float->int is x86's integer indefinite by
// construction (the host saturates), so the model is checked there against the rule itself.
#include <stdio.h>
#include <string.h>
#include <fenv.h>
#include <math.h>
#include "../softref-cvt.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static uint32_t rf(void) {
    uint32_t x = (uint32_t)rnd();
    switch (rnd() & 7) {
    case 0: x &= 0x807FFFFF; break;
    case 1: { int E = 127 + (int)(rnd() % 26); uint32_t sig = ((uint32_t)rnd() & 0x7FFFFF); int sh = 150 - E; x = (x & 0x80000000u) | ((uint32_t)E << 23) | sig;
              if (sh >= 1 && sh <= 23 && (rnd() & 1)) { x &= ~((1u << sh) - 1); x |= 1u << (sh - 1); } break; }
    case 2: x = (x & 0x80000000u) | ((uint32_t)(150 + rnd() % 12) << 23) | ((uint32_t)rnd() & 0x7FFFFF); break;
    case 3: x = (x & 0x80000000u) | ((uint32_t)(rnd() % 3 ? 0x4EFFFFF0u : 0x5EFFFFF0u) + (uint32_t)(rnd() % 32)); break;
    default: break;
    }
    return x;
}
static uint64_t rd(void) {
    uint64_t x = rnd();
    switch (rnd() & 7) {
    case 0: x &= 0x800FFFFFFFFFFFFFull; break;
    case 1: x = (x & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(1023 + rnd() % 66) << 52); break;
    case 2: x = (x & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(1023 - 160 + rnd() % 300) << 52);
            if (rnd() & 1) { x = (x & ~0x1FFFFFFFull) | 0x10000000ull; } break;
    default: break;
    }
    return x;
}
int main(void) {
    int modes[4] = {FE_TONEAREST, FE_DOWNWARD, FE_UPWARD, FE_TOWARDZERO}, bad = 0; long n = 0;
    for (int rm = 0; rm < 4; rm++) {
        fesetround(modes[rm]);
        for (long i = 0; i < 3000000; i++) {
            uint32_t xb = rf(); float xf; memcpy(&xf, &xb, 4);
            uint64_t db = rd(); double xd; memcpy(&xd, &db, 8);
            // float -> int32/int64, nearest per mode (nearbyint follows the current mode) and truncating
            if (isfinite(xf)) {
                uint64_t o; int ind;
                ind = sf_f2i(F32, xb, rm, 32, &o); n++;
                if (fabsf(xf) < 2147483648.0f) {
                    float r = nearbyintf(xf);
                    if (r >= -2147483648.0f && r < 2147483648.0f) { int32_t h = (int32_t)r; if (ind || (uint32_t)h != (uint32_t)o) { if (bad++ < 10) printf("f2i32 rm=%d %08x host %08x model %08llx ind=%d\n", rm, xb, (uint32_t)h, (unsigned long long)o, ind); } }
                    else if (!ind && bad++ < 10) printf("f2i32 rm=%d %08x should be indefinite\n", rm, xb);
                    int32_t t = (int32_t)xf; ind = sf_f2i(F32, xb, 4, 32, &o);
                    if (ind || (uint32_t)t != (uint32_t)o) { if (bad++ < 10) printf("f2i32 trunc %08x host %08x model %08llx\n", xb, (uint32_t)t, (unsigned long long)o); }
                } else { if (!ind && !(xf == -2147483648.0f) && bad++ < 10) printf("f2i32 %08x should be indefinite\n", xb); }
                ind = sf_f2i(F32, xb, rm, 64, &o);
                if (fabsf(xf) < 9.2e18f) { long long h = llrintf(xf); if (ind || (uint64_t)h != o) { if (bad++ < 10) printf("f2i64 rm=%d %08x host %016llx model %016llx\n", rm, xb, (unsigned long long)h, (unsigned long long)o); } }
            } else { uint64_t o; if (!sf_f2i(F32, xb, rm, 32, &o) || o != 0x80000000ull) { if (bad++ < 10) printf("f2i nan/inf %08x\n", xb); } }
            if (isfinite(xd)) {
                uint64_t o; int ind = sf_f2i(F64, db, rm, 32, &o); n++;
                if (fabs(xd) < 2147483647.0) { int32_t h = (int32_t)nearbyint(xd); if (ind || (uint32_t)h != (uint32_t)o) { if (bad++ < 10) printf("d2i32 rm=%d %016llx host %08x model %08llx\n", rm, (unsigned long long)db, (uint32_t)h, (unsigned long long)o); } }
                ind = sf_f2i(F64, db, rm, 64, &o);
                if (fabs(xd) < 9.2e18) { long long h = llrint(xd); if (ind || (uint64_t)h != o) { if (bad++ < 10) printf("d2i64 rm=%d %016llx host %016llx model %016llx\n", rm, (unsigned long long)db, (unsigned long long)h, (unsigned long long)o); } }
            }
            // int -> float / double
            int64_t v = (int64_t)rnd() >> (rnd() % 64); int32_t v32 = (int32_t)v;
            { volatile float hf = (float)v32; uint32_t hb; float t = hf; memcpy(&hb, &t, 4); uint32_t m = (uint32_t)sf_i2f(F32, v32, rm); n++; if (hb != m && bad++ < 10) printf("i32->f rm=%d %d host %08x model %08x\n", rm, v32, hb, m); }
            { volatile float hf = (float)v; uint32_t hb; float t = hf; memcpy(&hb, &t, 4); uint32_t m = (uint32_t)sf_i2f(F32, v, rm); if (hb != m && bad++ < 10) printf("i64->f rm=%d %lld host %08x model %08x\n", rm, (long long)v, hb, m); }
            { volatile double hd = (double)v; uint64_t hb; double t = hd; memcpy(&hb, &t, 8); uint64_t m = sf_i2f(F64, v, rm); if (hb != m && bad++ < 10) printf("i64->d rm=%d %lld host %016llx model %016llx\n", rm, (long long)v, (unsigned long long)hb, (unsigned long long)m); }
            // float <-> double
            if (!isnan(xf)) { volatile double hd = (double)xf; double t = hd; uint64_t hb; memcpy(&hb, &t, 8); uint64_t m = sf_f2f(F32, F64, xb, rm, 0, 0); if (hb != m && bad++ < 10) printf("f->d %08x host %016llx model %016llx\n", xb, (unsigned long long)hb, (unsigned long long)m); }
            if (!isnan(xd)) { volatile float hf = (float)xd; float t = hf; uint32_t hb; memcpy(&hb, &t, 4); uint32_t m = (uint32_t)sf_f2f(F64, F32, db, rm, 0, 0); if (hb != m && bad++ < 10) printf("d->f rm=%d %016llx host %08x model %08x\n", rm, (unsigned long long)db, hb, m); }
        }
    }
    // NaN rules and DAZ/FTZ by construction
    if (sf_f2f(F64, F32, 0x7FF0000000000001ull, 0, 0, 0) != 0x7FC00000u) { bad++; printf("snan d->f\n"); }
    if (sf_f2f(F32, F64, 0xFF800001u, 0, 0, 0) != 0xFFF8000020000000ull) { bad++; printf("snan f->d %016llx\n", (unsigned long long)sf_f2f(F32, F64, 0xFF800001u, 0, 0, 0)); }
    { uint64_t o; sf_f2i(F32, 0x00000001u, 2, 32, &o); if (o != 1) { bad++; printf("up of tiny positive must be 1\n"); } sf_f2i(F32, sf_daz(F32, 1u), 2, 32, &o); if (o != 0) { bad++; printf("DAZ tiny must be 0\n"); } }
    if (sf_f2f(F64, F32, 0x3800000000000000ull, 0, 0, 1) != 0) { bad++; printf("FTZ d->f tiny must be +0\n"); }
    printf("softref-cvt: %ld cases checked against the host FPU, %d mismatches\n", n, bad);
    return bad != 0;
}
