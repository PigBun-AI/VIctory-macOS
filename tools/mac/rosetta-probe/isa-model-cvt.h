// Software model of the AVX conversions between float32, float64 and int32/int64 (vcvtps2dq/vcvttps2dq/vcvtdq2ps/vcvtpd2dq/vcvttpd2dq/vcvtpd2ps/vcvtps2pd/vcvtdq2pd,
// vcvtss2sd/vcvtsd2ss, vcvt[t]ss2si/vcvt[t]sd2si, vcvtsi2ss/vcvtsi2sd) under every MXCSR rounding mode, DAZ and FTZ. Integer-only; built on softref.h.
// Out-of-range and NaN sources give the integer indefinite (the most negative value); NaN payloads are carried and quieted.
#ifndef ISA_MODEL_CVT_H
#define ISA_MODEL_CVT_H
#include "isa-model-fp.h"
#include "isa-model-shuf.h"
enum { CV_F2I_V, CV_I2F_V, CV_PD2I, CV_PD2PS, CV_PS2PD, CV_DQ2PD, CV_F2F_S, CV_F2I_S, CV_I2F_S };
// op->p: kind | trunc << 8 | bits << 16 (GPR width) | dbl << 24 (source/destination double for the scalar forms: F2F_S dbl = source is double; I2F_S dbl = destination is double; F2I_S dbl = source is double)
static uint64_t cv_f2i(Fmt f, uint64_t x, int bits, int trunc, Fpc c) {
    uint64_t indef = 1ull << (bits - 1), mask = bits == 64 ? ~0ull : (1ull << bits) - 1;
    if (c.daz) x = sf_daz(f, x);
    Unp u = sf_unpack(f, x);
    if (u.cls >= 2) return indef;
    if (u.cls == 0) return 0;
    u128 kept; int rbit = 0, stk = 0;
    if (u.e >= 0) { if (u.e >= 64) return indef; kept = (u128)u.sig << u.e; }
    else { int sh = -u.e; if (sh >= 127) { kept = 0; stk = 1; } else { kept = (u128)u.sig >> sh; u128 m = (u128)u.sig & (((u128)1 << sh) - 1); rbit = (int)((m >> (sh - 1)) & 1); stk = (m & (((u128)1 << (sh - 1)) - 1)) != 0; } }
    int rm = trunc ? 3 : c.rm, inexact = rbit || stk, up = 0;
    switch (rm) { case 0: up = rbit && (stk || (int)(kept & 1)); break; case 1: up = u.sign && inexact; break; case 2: up = !u.sign && inexact; break; default: up = 0; }
    kept += (u128)up;
    u128 lim = (u128)1 << (bits - 1);
    if (u.sign) { if (kept > lim) return indef; return (uint64_t)(0 - (uint64_t)kept) & mask; }
    if (kept >= lim) return indef;
    return (uint64_t)kept;
}
static uint64_t cv_i2f(Fmt f, int64_t v, Fpc c) { int s = v < 0; u128 mag = s ? (u128)0 - (u128)(__int128)v : (u128)v; return sf_round(f, s, mag, 0, 0, c.rm); }
static uint64_t cv_f2f(int dbl_src, uint64_t x, Fpc c) {
    Fmt fs = dbl_src ? F64 : F32, fd = dbl_src ? F32 : F64;
    if (sf_isnan(fs, x)) {
        uint64_t sgn = x >> (fs.eb + fs.mb), mant = x & ((1ull << fs.mb) - 1), E = (uint64_t)((1 << fd.eb) - 1);
        uint64_t m2 = dbl_src ? (mant >> 29) : (mant << 29); m2 |= 1ull << (fd.mb - 1);
        return (sgn << (fd.eb + fd.mb)) | (E << fd.mb) | m2;
    }
    if (c.daz) x = sf_daz(fs, x);
    Unp u = sf_unpack(fs, x);
    if (u.cls == 2) return sf_inf(fd, u.sign);
    if (u.cls == 0) return (uint64_t)u.sign << (fd.eb + fd.mb);
    sf_ftz = dbl_src ? c.ftz : 0; uint64_t r = sf_round(fd, u.sign, (u128)u.sig, u.e, 0, c.rm); sf_ftz = 0; return r;
}
static void m_cvt(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, kind = pv & 0xFF, trunc = (pv >> 8) & 1, bits = (pv >> 16) & 0xFF, dbl = (pv >> 24) & 1, w = c->w; Fpc fc = fpc_of(c->mx);
    int al = c->op->alias; const uint8_t *a = r->a, *src2 = (al == AL_AB || al == AL_ALL) ? r->a : al == AL_MEM ? msrc(r, c->op) : r->b, *s1 = (al == AL_DA && kind != CV_F2F_S && kind != CV_I2F_S) ? r->a : src2; uint8_t *d = o->r; memset(d, 0, 32);
    switch (kind) {
    case CV_F2I_V: for (int i = 0; i < w / 4; i++) se_(d, i, 4, cv_f2i(F32, el_(s1, i, 4), 32, trunc, fc)); return;
    case CV_I2F_V: for (int i = 0; i < w / 4; i++) se_(d, i, 4, cv_i2f(F32, (int32_t)el_(s1, i, 4), fc)); return;
    case CV_PD2I: for (int i = 0; i < w / 8; i++) se_(d, i, 4, cv_f2i(F64, el_(s1, i, 8), 32, trunc, fc)); return;
    case CV_PD2PS: for (int i = 0; i < w / 8; i++) se_(d, i, 4, cv_f2f(1, el_(s1, i, 8), fc)); return;
    case CV_PS2PD: for (int i = 0; i < w / 8; i++) se_(d, i, 8, cv_f2f(0, el_(s1, i, 4), fc)); return;
    case CV_DQ2PD: for (int i = 0; i < w / 8; i++) se_(d, i, 8, cv_i2f(F64, (int32_t)el_(s1, i, 4), fc)); return;
    case CV_F2F_S: memcpy(d, a, 16); if (dbl) se_(d, 0, 4, cv_f2f(1, el_(s1, 0, 8), fc)); else se_(d, 0, 8, cv_f2f(0, el_(s1, 0, 4), fc)); return;
    case CV_F2I_S: o->x[0] = cv_f2i(dbl ? F64 : F32, el_(s1, 0, dbl ? 8 : 4), bits, trunc, fc); return;
    case CV_I2F_S: { int64_t v = bits == 64 ? (int64_t)el_(s1, 0, 8) : (int64_t)(int32_t)el_(s1, 0, 4); memcpy(d, a, 16); if (dbl) se_(d, 0, 8, cv_i2f(F64, v, fc)); else se_(d, 0, 4, cv_i2f(F32, v, fc)); return; }
    }
}
#endif
