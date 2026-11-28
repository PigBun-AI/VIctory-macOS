// Software reference model for the x86 conversion instructions (CVTT/CVT PS/PD/SS/SD <-> DWORD/QWORD, CVTDQ2PS/PD, CVTSI2SS/SD,
// CVTPS2PD, CVTPD2PS, CVTSS2SD, CVTSD2SS), plain integer C on top of softref.h, so the expected bits do not depend on the machine
// under test. Rounding mode rm: 0 nearest-even, 1 down, 2 up, 3 toward zero (MXCSR.RC); 4 = truncating instruction (CVTT*, ignores RC).
// x86 semantics (Intel SDM / AMD APM, identical here):
//   float/double -> integer: NaN, infinity, or a value that is out of range after rounding gives the "integer indefinite"
//     (0x80000000 / 0x8000000000000000); Arm's FCVTZS/FCVTNS would saturate and map NaN to 0.
//   DAZ=1: a denormal source is read as zero of the same sign (so CVTPS2DQ with RC=up of a tiny positive value gives 0, not 1).
//   FTZ=1: a tiny result of CVTPD2PS/CVTSD2SS (below the smallest normal after rounding, unbounded exponent) becomes a signed zero.
//   NaN -> wider/narrower float: quiet NaN keeping the sign and the top payload bits; infinity and zero keep their sign.
//   integer -> float: rounded per RC; 0 gives +0 in every mode; CVTSI2SS to a double-to-single narrowing overflows per RC.
// tests/validate-cvt.c compares these models with the arm64/x86 host FPU in the range where the host agrees with x86.
#ifndef SOFTREF_CVT_H
#define SOFTREF_CVT_H
#include "softref.h"

// DAZ: a denormal input is a zero of the same sign
static inline uint64_t sf_daz(Fmt f, uint64_t x) {
    uint64_t E = (x >> f.mb) & ((1u << f.eb) - 1), m = x & ((1ull << f.mb) - 1);
    return (E == 0 && m) ? (x & (1ull << (f.eb + f.mb))) : x;
}

// float/double bits -> signed integer of ibits (32 or 64). Returns 1 when x86 gives the integer indefinite; *out holds the bits.
static inline int sf_f2i(Fmt f, uint64_t x, int rm, int ibits, uint64_t *out) {
    uint64_t indef = (uint64_t)1 << (ibits - 1);
    Unp u = sf_unpack(f, x);
    if (u.cls >= 2) { *out = indef; return 1; }
    if (u.cls == 0) { *out = 0; return 0; }
    u128 q; int rbit = 0, stk = 0;
    if (u.e >= 0) {
        if (u.e > 70) { *out = indef; return 1; }
        q = (u128)u.sig << u.e;
    } else {
        int sh = -u.e;
        if (sh >= 127) { q = 0; stk = 1; }
        else {
            q = (u128)u.sig >> sh; u128 rem = (u128)u.sig & (((u128)1 << sh) - 1);
            rbit = (int)((rem >> (sh - 1)) & 1); stk = (rem & (((u128)1 << (sh - 1)) - 1)) != 0;
        }
    }
    int inexact = rbit || stk, up = 0;
    switch (rm) { case 0: up = rbit && (stk || (int)(q & 1)); break; case 1: up = u.sign && inexact; break; case 2: up = !u.sign && inexact; break; default: up = 0; }
    q += up;
    u128 maxp = ((u128)1 << (ibits - 1)) - 1;
    if (!u.sign ? q > maxp : q > maxp + 1) { *out = indef; return 1; }
    uint64_t mag = (uint64_t)q;
    *out = u.sign ? (uint64_t)0 - mag : mag;
    if (ibits == 32) *out &= 0xFFFFFFFFull;
    return 0;
}

// signed integer (sign-extended into int64) -> float/double bits
static inline uint64_t sf_i2f(Fmt f, int64_t v, int rm) {
    if (v == 0) return 0;
    int s = v < 0; uint64_t mag = s ? (uint64_t)0 - (uint64_t)v : (uint64_t)v;
    return sf_round(f, s, mag, 0, 0, rm);
}

// float <-> double. daz flushes a denormal source, ftz flushes a tiny result (sign kept).
static inline uint64_t sf_f2f(Fmt from, Fmt to, uint64_t x, int rm, int daz, int ftz) {
    if (daz) x = sf_daz(from, x);
    Unp u = sf_unpack(from, x);
    int sb = to.eb + to.mb;
    if (u.cls == 3) {
        uint64_t m = x & ((1ull << from.mb) - 1), nm;
        if (to.mb > from.mb) nm = m << (to.mb - from.mb); else nm = m >> (from.mb - to.mb);
        return ((uint64_t)u.sign << sb) | sf_inf(to, 0) | (1ull << (to.mb - 1)) | nm;
    }
    if (u.cls == 2) return sf_inf(to, u.sign);
    if (u.cls == 0) return (uint64_t)u.sign << sb;
    if (ftz) {
        sf_unb = 1; sf_unb_eres = 0x7fffffff; (void)sf_round(to, u.sign, u.sig, u.e, 0, rm); sf_unb = 0;
        if (sf_unb_eres < 1 - ((1 << (to.eb - 1)) - 1)) return (uint64_t)u.sign << sb;
    }
    return sf_round(to, u.sign, u.sig, u.e, 0, rm);
}
#endif
