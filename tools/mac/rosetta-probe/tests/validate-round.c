// Checks rnd_int (isa-round.h) against arm64 frintn/frintm/frintp/frintz on float32 and float64:  clang -O1 -I.. validate-round.c -o v && ./v
#include <math.h>
#include "../isa-model-round.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
int main(void) {
    long bad = 0, total = 0;
    for (long it = 0; it < 40000000; it++) {
        int dbl = (int)(rnd() & 1), es = dbl ? 8 : 4, fb = dbl ? 52 : 23, bias = dbl ? 1023 : 127; uint64_t m = rnd() & ((1ull << fb) - 1), sg = rnd() & 1; int k = (int)(rnd() % (unsigned)(fb + 8)) - 6; uint64_t E = (uint64_t)(bias + k);
        int nb = k >= 0 && k < fb ? fb - k : fb; uint64_t low = (1ull << nb) - 1; switch (rnd() % 5) { case 0: m &= ~low; break; case 1: m = (m & ~low) | (1ull << (nb - 1)); break; case 2: m = (m & ~low) | 1; break; case 3: m |= low; break; default: break; }
        if (rnd() % 50 == 0) E = (1ull << (dbl ? 11 : 8)) - 1; if (rnd() % 50 == 0) E = 0;
        uint64_t x = (sg << (es * 8 - 1)) | (E << fb) | m; int rm = (int)(rnd() & 3), imm = rm; Fpc c = {0, 0, 0}; uint64_t want = rnd_int(dbl ? F64 : F32, x, imm, c), ref;
        if (dbl) { double d, r; memcpy(&d, &x, 8); switch (rm) { case 0: __asm__("frintn %d0,%d1" : "=w"(r) : "w"(d)); break; case 1: __asm__("frintm %d0,%d1" : "=w"(r) : "w"(d)); break; case 2: __asm__("frintp %d0,%d1" : "=w"(r) : "w"(d)); break; default: __asm__("frintz %d0,%d1" : "=w"(r) : "w"(d)); } memcpy(&ref, &r, 8); }
        else { float f, r; uint32_t x32 = (uint32_t)x; memcpy(&f, &x32, 4); switch (rm) { case 0: __asm__("frintn %s0,%s1" : "=w"(r) : "w"(f)); break; case 1: __asm__("frintm %s0,%s1" : "=w"(r) : "w"(f)); break; case 2: __asm__("frintp %s0,%s1" : "=w"(r) : "w"(f)); break; default: __asm__("frintz %s0,%s1" : "=w"(r) : "w"(f)); } uint32_t r32; memcpy(&r32, &r, 4); ref = r32; }
        total++; int nan = sf_isnan(dbl ? F64 : F32, x); if (nan) { if (!sf_isnan(dbl ? F64 : F32, want) || !((want >> (fb - 1)) & 1)) bad++; continue; }
        if (want != ref) { if (bad++ < 10) printf("x=%016llx rm=%d want=%016llx ref=%016llx\n", (unsigned long long)x, rm, (unsigned long long)want, (unsigned long long)ref); }
    }
    printf("validate-round: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
