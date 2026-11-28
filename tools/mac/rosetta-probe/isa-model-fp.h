// Software model of the x86 SSE/AVX floating point arithmetic (add, sub, mul, div, sqrt, max, min; ps/pd/ss/sd) on top of softref.h:
// integer-only IEEE binary32/64 with the four MXCSR rounding modes, DAZ and FTZ, and the x86 NaN rules. Portable C (validated on arm64 by tests/validate-fp.c).
//   NaN operands: a NaN in src1 is returned (quieted) if there is one, else the NaN in src2 (quieted); an invalid operation without a NaN
//     operand (inf-inf, 0*inf, 0/0, inf/inf, sqrt of a negative number) gives the default NaN (negative quiet NaN, 0xFFC00000 / 0xFFF8...).
//   MAX/MIN: a > b ? a : b (a < b ? a : b): if either is a NaN, or both are zeros of either sign, src2 is returned unchanged (a signalling NaN stays signalling).
//   DAZ: denormal inputs are read as zero of the same sign (also for the operand MAX/MIN return); FTZ: a result that is tiny after rounding becomes a signed zero.
#ifndef ISA_MODEL_FP_H
#define ISA_MODEL_FP_H
#include "isa-types.h"
#include "softref-cvt.h"
#ifndef MODEL_NEG
#define MODEL_NEG 0
#endif
typedef struct { int rm, daz, ftz; } Fpc;
static inline Fpc fpc_of(uint32_t mx) { Fpc c = {(int)((mx >> 13) & 3), !!(mx & 0x40), !!(mx & 0x8000)}; return c; }
enum { FP_ADD, FP_SUB, FP_MUL, FP_DIV, FP_MAX, FP_MIN, FP_SQRT };
static inline uint64_t fp_quiet(Fmt f, uint64_t x) { return x | (1ull << (f.mb - 1)); }
static inline uint64_t fp_one(Fmt f) { return (uint64_t)((1 << (f.eb - 1)) - 1) << f.mb; }
static inline int fp_iszero(Fmt f, uint64_t x) { return (x & ~(1ull << (f.eb + f.mb))) == 0; }
// a < b for non-NaN values (zeros of both signs are equal)
static inline int fp_lt(Fmt f, uint64_t a, uint64_t b) {
    uint64_t sm = 1ull << (f.eb + f.mb), ma = a & ~sm, mb = b & ~sm; int sa = !!(a & sm), sb = !!(b & sm);
    if (ma == 0 && mb == 0) return 0;
    if (sa != sb) return sa;
    return sa ? ma > mb : ma < mb;
}
static inline u128 fp_isqrt(u128 n) {          // floor(sqrt(n)) for n < 2^126
    u128 res = 0, bit = (u128)1 << 124;
    while (bit > n) bit >>= 2;
    while (bit) { if (n >= res + bit) { n -= res + bit; res = (res >> 1) + bit; } else res >>= 1; bit >>= 2; }
    return res;
}
static uint64_t fp_div(Fmt f, uint64_t a, uint64_t b, int rm) {
    Unp UA = sf_unpack(f, a), UB = sf_unpack(f, b); int s = UA.sign ^ UB.sign; uint64_t sg = (uint64_t)s << (f.eb + f.mb);
    if (UA.cls == 2) return UB.cls == 2 ? sf_dnan(f) : sf_inf(f, s);
    if (UB.cls == 2) return sg;
    if (UA.cls == 0) return UB.cls == 0 ? sf_dnan(f) : sg;
    if (UB.cls == 0) return sf_inf(f, s);
    int na = 63 - sf_msb(UA.sig), nb = 63 - sf_msb(UB.sig); u128 sa = (u128)UA.sig << na, sb = (u128)UB.sig << nb;
    u128 num = sa << 62, q = num / sb; int sticky = (num - q * sb) != 0;
    return sf_round(f, s, q, (UA.e - na) - (UB.e - nb) - 62, sticky, rm);
}
static uint64_t fp_sqrt(Fmt f, uint64_t a, int rm) {
    Unp UA = sf_unpack(f, a);
    if (UA.cls == 0) return a;
    if (UA.sign) return sf_dnan(f);
    if (UA.cls == 2) return a;
    int k = 120 - sf_msb(UA.sig); u128 n = (u128)UA.sig << k; int e = UA.e - k;
    if (e & 1) { n <<= 1; e -= 1; }
    u128 r = fp_isqrt(n); int sticky = r * r != n;
    return sf_round(f, 0, r, e / 2, sticky, rm);
}
static uint64_t fp_op(int kind, Fmt f, uint64_t a, uint64_t b, Fpc c) {
    if (kind == FP_MAX || kind == FP_MIN) {
        if (c.daz) { a = sf_daz(f, a); b = sf_daz(f, b); }
        if (sf_isnan(f, a) || sf_isnan(f, b)) return b;
        if (MODEL_NEG) return kind == FP_MAX ? (fp_lt(f, b, a) || !fp_lt(f, a, b) ? a : b) : (fp_lt(f, b, a) || !fp_lt(f, a, b) ? b : a);      // negative control: Arm-style (a wins ties; NaN is not handled)
        return kind == FP_MAX ? (fp_lt(f, b, a) ? a : b) : (fp_lt(f, a, b) ? a : b);
    }
    if (kind == FP_SQRT) {
        if (sf_isnan(f, b)) return fp_quiet(f, b);
        if (c.daz) b = sf_daz(f, b);
        uint64_t r; sf_ftz = c.ftz; r = fp_sqrt(f, b, c.rm); sf_ftz = 0; return r;
    }
    if (sf_isnan(f, a)) return fp_quiet(f, a);
    if (sf_isnan(f, b)) return fp_quiet(f, b);
    if (c.daz) { a = sf_daz(f, a); b = sf_daz(f, b); }
    uint64_t r, nz = 1ull << (f.eb + f.mb); sf_ftz = c.ftz;
    switch (kind) {
    case FP_ADD: r = sf_fma(f, a, fp_one(f), b, 0, 0, c.rm); break;
    case FP_SUB: r = sf_fma(f, a, fp_one(f), b, 0, 1, c.rm); break;
    case FP_MUL: r = sf_fma(f, a, b, (a ^ b) & nz, 0, 0, c.rm); break;      // c = a zero of the product's sign: an exact zero product keeps its sign in every rounding mode
    default: r = fp_div(f, a, b, c.rm); break;
    }
    sf_ftz = 0; return r;
}
// op parameter: kind | dbl << 8 | scalar << 16
static void m_fparith(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, kind = pv & 0xFF, dbl = (pv >> 8) & 1, sc = (pv >> 16) & 1; Fmt f = dbl ? F64 : F32; int sz = dbl ? 8 : 4; Fpc fc = fpc_of(c->mx);
    const uint8_t *a = r->a, *b = c->op->alias == AL_MEM ? msrc(r, c->op) : r->b;
    memset(o->r, 0, 32);
    if (kind == FP_SQRT && !sc) { const uint8_t *s = c->op->alias == AL_DA ? r->a : b; for (int i = 0; i < c->w / sz; i++) st_(o->r + i * sz, sz, fp_op(kind, f, 0, ld_(s + i * sz, sz), fc)); return; }
    if (sc) { memcpy(o->r, r->a, 16); st_(o->r, sz, fp_op(kind, f, ld_(a, sz), ld_(b, sz), fc)); return; }
    for (int i = 0; i < c->w / sz; i++) st_(o->r + i * sz, sz, fp_op(kind, f, ld_(a + i * sz, sz), ld_(b + i * sz, sz), fc));
}
#endif
