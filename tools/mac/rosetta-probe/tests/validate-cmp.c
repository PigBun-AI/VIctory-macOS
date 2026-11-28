// Checks isa-model-cmp.h (relation of two floats, the 32 vcmp predicates, the ucomis flags) against arm64 FCMP flags (N Z C V of fcmp: less 1000, equal 0110, greater 0010,
// unordered 0011) and the C comparison operators:  clang -O1 -I.. validate-cmp.c -o v && ./v
#include "../isa-model-cmp.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static uint32_t g32(void) { static const uint32_t e[] = {0, 0x80000000u, 1, 0x80000001u, 0x007FFFFF, 0x00800000, 0x3F800000, 0xBF800000, 0x7F7FFFFF, 0x7F800000, 0xFF800000, 0x7FC00000, 0x7F800001, 0xFFC00001, 0xFF7FFFFF}; return rnd() % 3 == 0 ? e[rnd() % 15] : (uint32_t)rnd(); }
static uint64_t g64(void) { static const uint64_t e[] = {0, 0x8000000000000000ull, 1, 0x8000000000000001ull, 0x000FFFFFFFFFFFFFull, 0x0010000000000000ull, 0x3FF0000000000000ull, 0xBFF0000000000000ull, 0x7FEFFFFFFFFFFFFFull, 0x7FF0000000000000ull, 0xFFF0000000000000ull, 0x7FF8000000000000ull, 0x7FF0000000000001ull, 0xFFF8000000000001ull}; return rnd() % 3 == 0 ? e[rnd() % 14] : rnd(); }
static long bad, total;
static void chk(const char *w, uint64_t a, uint64_t b, uint64_t got, uint64_t want) { total++; if (got != want) { if (bad++ < 12) printf("%s a=%llx b=%llx got=%llx want=%llx\n", w, (unsigned long long)a, (unsigned long long)b, (unsigned long long)got, (unsigned long long)want); } }
static int hw_rel32(uint32_t a, uint32_t b) { uint64_t nzcv; float x, y; memcpy(&x, &a, 4); memcpy(&y, &b, 4); __asm__("fcmp %s1, %s2\n mrs %0, nzcv" : "=r"(nzcv) : "w"(x), "w"(y) : "cc"); unsigned f = (unsigned)(nzcv >> 28) & 15; return f == 8 ? 0 : f == 6 ? 1 : f == 2 ? 2 : 3; }
static int hw_rel64(uint64_t a, uint64_t b) { uint64_t nzcv; double x, y; memcpy(&x, &a, 8); memcpy(&y, &b, 8); __asm__("fcmp %d1, %d2\n mrs %0, nzcv" : "=r"(nzcv) : "w"(x), "w"(y) : "cc"); unsigned f = (unsigned)(nzcv >> 28) & 15; return f == 8 ? 0 : f == 6 ? 1 : f == 2 ? 2 : 3; }
int main(void) {
    for (long it = 0; it < 20000000; it++) {
        uint32_t a = g32(), b = g32(); if (rnd() % 4 == 0) b = a; uint64_t A = g64(), B = g64(); if (rnd() % 4 == 0) B = A;
        float fa, fb; memcpy(&fa, &a, 4); memcpy(&fb, &b, 4); double da, db; memcpy(&da, &A, 8); memcpy(&db, &B, 8);
        int r32 = fp_rel(F32, a, b, 0), r64 = fp_rel(F64, A, B, 0); chk("rel32", a, b, (uint64_t)r32, (uint64_t)hw_rel32(a, b)); chk("rel64", A, B, (uint64_t)r64, (uint64_t)hw_rel64(A, B));
        // DAZ: compare after flushing with the model's own flush; the hardware compares the flushed values
        { uint32_t fa2 = (uint32_t)sf_daz(F32, a), fb2 = (uint32_t)sf_daz(F32, b); chk("rel32 daz", a, b, (uint64_t)fp_rel(F32, a, b, 1), (uint64_t)hw_rel32(fa2, fb2)); }
        for (int p = 0; p < 32; p++) {       // predicate truth table from the C operators
            int want; switch (p & 15) { case 0: want = fa == fb; break; case 1: want = fa < fb; break; case 2: want = fa <= fb; break; case 3: want = fa != fa || fb != fb; break; case 4: want = !(fa == fb); break; case 5: want = !(fa < fb); break; case 6: want = !(fa <= fb); break;
            case 7: want = !(fa != fa || fb != fb); break; case 8: want = fa == fb || fa != fa || fb != fb; break; case 9: want = !(fa >= fb); break; case 10: want = !(fa > fb); break; case 11: want = 0; break; case 12: want = fa != fb && fa == fa && fb == fb; break;
            case 13: want = fa >= fb; break; case 14: want = fa > fb; break; default: want = 1; }
            chk("pred", a, b, (uint64_t)cmp_pred(p, r32), (uint64_t)want);
            double xa = da, xb = db; int w64; switch (p & 15) { case 0: w64 = xa == xb; break; case 1: w64 = xa < xb; break; case 2: w64 = xa <= xb; break; case 3: w64 = xa != xa || xb != xb; break; case 4: w64 = !(xa == xb); break; case 5: w64 = !(xa < xb); break; case 6: w64 = !(xa <= xb); break;
            case 7: w64 = !(xa != xa || xb != xb); break; case 8: w64 = xa == xb || xa != xa || xb != xb; break; case 9: w64 = !(xa >= xb); break; case 10: w64 = !(xa > xb); break; case 11: w64 = 0; break; case 12: w64 = xa != xb && xa == xa && xb == xb; break;
            case 13: w64 = xa >= xb; break; case 14: w64 = xa > xb; break; default: w64 = 1; }
            chk("pred64", A, B, (uint64_t)cmp_pred(p, r64), (uint64_t)w64);
        }
    }
    printf("validate-cmp: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
