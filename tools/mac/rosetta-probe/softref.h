// Software reference model for the Rosetta-under-Wine probe (probe-wine.c): IEEE binary16/32/64 fused
// multiply-add and conversions with the four x86 rounding modes (MXCSR.RC / imm8[1:0] encoding:
// 0 nearest-even, 1 down, 2 up, 3 toward zero), in plain integer C (no FPU, no libm), so the expected
// bits do not depend on the machine under test. Subnormal inputs/outputs are exact (no DAZ/FTZ).
// tests/validate-softref.c compares it with the host FPU (arm64 or x86) when built natively.
#ifndef SOFTREF_H
#define SOFTREF_H
#include <stdint.h>

typedef unsigned __int128 u128;
typedef struct { int eb, mb; } Fmt;
static const Fmt F16 = {5, 10}, F32 = {8, 23}, F64 = {11, 52};

static inline int sf_msb(u128 x) {            // index of the highest set bit, x != 0
    uint64_t hi = (uint64_t)(x >> 64), lo = (uint64_t)x;
    return hi ? 127 - __builtin_clzll(hi) : 63 - __builtin_clzll(lo);
}
static inline uint64_t sf_inf(Fmt f, int s) { return ((uint64_t)s << (f.eb + f.mb)) | ((uint64_t)((1 << f.eb) - 1) << f.mb); }
static inline uint64_t sf_dnan(Fmt f) { return sf_inf(f, 1) | (1ull << (f.mb - 1)); }   // x86 "real indefinite"
static inline int sf_isnan(Fmt f, uint64_t x) {
    uint64_t e = (x >> f.mb) & ((1u << f.eb) - 1), m = x & ((1ull << f.mb) - 1);
    return e == (uint64_t)((1 << f.eb) - 1) && m != 0;
}
static inline int sf_issnan(Fmt f, uint64_t x) { return sf_isnan(f, x) && !((x >> (f.mb - 1)) & 1); }

typedef struct { int sign, cls; uint64_t sig; int e; } Unp;   // cls 0 zero, 1 finite, 2 inf, 3 nan; value = sig * 2^e
static inline Unp sf_unpack(Fmt f, uint64_t x) {
    Unp u; int bias = (1 << (f.eb - 1)) - 1;
    uint64_t E = (x >> f.mb) & ((1u << f.eb) - 1), m = x & ((1ull << f.mb) - 1);
    u.sign = (int)(x >> (f.eb + f.mb)) & 1; u.sig = 0; u.e = 0;
    if (E == (uint64_t)((1 << f.eb) - 1)) { u.cls = m ? 3 : 2; return u; }
    if (E == 0) { u.cls = m ? 1 : 0; u.sig = m; u.e = 1 - bias - f.mb; return u; }
    u.cls = 1; u.sig = m | (1ull << f.mb); u.e = (int)E - bias - f.mb; return u;
}

// sf_unb=1 makes sf_round report, in sf_unb_eres, the exponent of the leading bit after rounding to the format's
// precision with an unbounded exponent range (x86 decides "tiny" for FTZ from that); it then returns 0.
static int sf_unb = 0, sf_unb_eres = 0x7fffffff;
// sf_ftz=1: MXCSR.FTZ - a result that is tiny after rounding with an unbounded exponent (x86 detects tininess after rounding) becomes a zero of the same sign
static int sf_ftz = 0;

// Rounds (r + eps) * 2^e0 (eps in [0,1) if sticky) to format f; the sign is given separately.
static uint64_t sf_round(Fmt f, int sign, u128 r, int e0, int sticky, int rm) {
    int bias = (1 << (f.eb - 1)) - 1, emax = (1 << f.eb) - 1, emin = 1 - bias;
    uint64_t sb = (uint64_t)sign << (f.eb + f.mb);
    if (r == 0) { if (sf_unb) sf_unb_eres = -(1 << 30); return sb; }
    if (sf_ftz && !sf_unb) { sf_unb = 1; sf_unb_eres = 0x7fffffff; (void)sf_round(f, sign, r, e0, sticky, rm); sf_unb = 0; if (sf_unb_eres < 1 - bias) return sb; }
    int n = sf_msb(r), eres = e0 + n;
    if (sf_unb) emin = -(1 << 28);
    int lsb = (eres > emin ? eres : emin) - f.mb, sh = lsb - e0;
    u128 kept; int rbit, stk;
    if (sh <= 0) { kept = r << (-sh); rbit = 0; stk = sticky; }
    else if (sh >= 127) { kept = 0; rbit = 0; stk = 1; }
    else {
        kept = r >> sh; u128 m = r & (((u128)1 << sh) - 1);
        rbit = (int)((m >> (sh - 1)) & 1); stk = ((m & (((u128)1 << (sh - 1)) - 1)) != 0) || sticky;
    }
    int inexact = rbit || stk, up = 0;
    switch (rm) { case 0: up = rbit && (stk || (int)(kept & 1)); break; case 1: up = sign && inexact; break; case 2: up = !sign && inexact; break; default: up = 0; }
    kept += up;
    if (sf_unb) { sf_unb_eres = (kept >> (f.mb + 1)) ? eres + 1 : eres; return 0; }
    int Eb = (eres > emin) ? eres + bias : 1;
    int to_inf = rm == 0 || (rm == 1 && sign) || (rm == 2 && !sign);
    uint64_t ovf = to_inf ? ((uint64_t)emax << f.mb) : (((uint64_t)(emax - 1) << f.mb) | ((1ull << f.mb) - 1));
    if (Eb >= emax) return sb | ovf;
    uint64_t bits = ((uint64_t)(Eb - 1) << f.mb) + (uint64_t)kept;
    if ((bits >> f.mb) >= (uint64_t)emax) return sb | ovf;
    return sb | bits;
}

// (neg_p ? -(a*b) : a*b) + (neg_c ? -c : c), single rounding. NaN operands are the caller's business.
static uint64_t sf_fma(Fmt f, uint64_t a, uint64_t b, uint64_t c, int neg_p, int neg_c, int rm) {
    Unp A = sf_unpack(f, a), B = sf_unpack(f, b), C = sf_unpack(f, c);
    int ps = A.sign ^ B.sign ^ neg_p, cs = C.sign ^ neg_c;
    int pinf = A.cls == 2 || B.cls == 2, pz = A.cls == 0 || B.cls == 0;
    if (pinf && pz) return sf_dnan(f);
    if (pinf) { if (C.cls == 2 && cs != ps) return sf_dnan(f); return sf_inf(f, ps); }
    if (C.cls == 2) return sf_inf(f, cs);
    uint64_t zero_sign;
    if (pz && C.cls == 0) { zero_sign = (ps == cs) ? (uint64_t)ps : (uint64_t)(rm == 1); return zero_sign << (f.eb + f.mb); }
    if (pz) return sf_round(f, cs, C.sig, C.e, 0, rm);
    u128 P = (u128)A.sig * B.sig; int eP = A.e + B.e;
    int sh = 120 - sf_msb(P); P <<= sh; eP -= sh;                   // msb at bit 120
    if (C.cls == 0) return sf_round(f, ps, P, eP, 0, rm);
    u128 Cs = (u128)C.sig; int eC = C.e; sh = 120 - sf_msb(Cs); Cs <<= sh; eC -= sh;
    u128 big, small; int sbig, ssmall, e0, d, sticky = 0;
    if (eP >= eC) { big = P; sbig = ps; small = Cs; ssmall = cs; e0 = eP; d = eP - eC; }
    else { big = Cs; sbig = cs; small = P; ssmall = ps; e0 = eC; d = eC - eP; }
    if (d >= 128) { sticky = small != 0; small = 0; }
    else if (d > 0) { sticky = (small & (((u128)1 << d) - 1)) != 0; small >>= d; }
    if (sbig == ssmall) return sf_round(f, sbig, big + small, e0, sticky, rm);
    if (d == 0) {
        if (big == small) return (uint64_t)(rm == 1) << (f.eb + f.mb);
        if (big > small) return sf_round(f, sbig, big - small, e0, 0, rm);
        return sf_round(f, ssmall, small - big, e0, 0, rm);
    }
    return sf_round(f, sbig, big - small - (sticky ? 1 : 0), e0, sticky, rm);
}

// float32 bits -> binary16 bits as VCVTPS2PH converts them (NaN -> quiet NaN keeping sign and the top payload bits)
static inline uint16_t sf_f2h(uint32_t x, int rm) {
    Unp u = sf_unpack(F32, x);
    if (u.cls == 3) return (uint16_t)((u.sign << 15) | 0x7C00 | 0x200 | ((x >> 13) & 0x3FF));
    if (u.cls == 2) return (uint16_t)sf_inf(F16, u.sign);
    if (u.cls == 0) return (uint16_t)(u.sign << 15);
    return (uint16_t)sf_round(F16, u.sign, u.sig, u.e, 0, rm);
}
// binary16 bits -> float32 bits as VCVTPH2PS converts them (exact; signalling NaN -> quiet NaN)
static inline uint32_t sf_h2f(uint16_t h) {
    Unp u = sf_unpack(F16, h);
    if (u.cls == 3) return ((uint32_t)u.sign << 31) | 0x7F800000u | 0x400000u | ((uint32_t)(h & 0x3FF) << 13);
    if (u.cls == 2) return (uint32_t)sf_inf(F32, u.sign);
    if (u.cls == 0) return (uint32_t)u.sign << 31;
    return (uint32_t)sf_round(F32, u.sign, u.sig, u.e, 0, 0);
}
#endif
