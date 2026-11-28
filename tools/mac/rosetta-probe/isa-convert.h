// Family avx_convert: the VEX conversions between float32, float64, int32 and int64 (packed xmm/ymm, scalar to and from general registers, scalar float<->float),
// register / destination-alias / memory-source forms, under every MXCSR rounding mode, DAZ and FTZ, checked bit for bit against isa-model-cvt.h.
// The generator concentrates float sources around the integer range limits (2^31, 2^63), halfway points, near-integers, and the float32 range edges for the narrowing forms.
#ifndef ISA_CONVERT_H
#define ISA_CONVERT_H
#include "isa-model-cvt.h"
#define G2C(M, MR, MM, RS, RD, W) \
    KRN(k_##M##_##W##dd, #MR " %%" RS "2,%%" RD "0", ST0) KRN(k_##M##_##W##da, #MR " %%" RS "1,%%" RD "1", ST1) KRN(k_##M##_##W##mm, #MM " 64(%0),%%" RD "0", ST0)
G2C(vcvtps2dq, vcvtps2dq, vcvtps2dq, "xmm", "xmm", X) G2C(vcvtps2dq, vcvtps2dq, vcvtps2dq, "ymm", "ymm", Y)
G2C(vcvttps2dq, vcvttps2dq, vcvttps2dq, "xmm", "xmm", X) G2C(vcvttps2dq, vcvttps2dq, vcvttps2dq, "ymm", "ymm", Y)
G2C(vcvtdq2ps, vcvtdq2ps, vcvtdq2ps, "xmm", "xmm", X) G2C(vcvtdq2ps, vcvtdq2ps, vcvtdq2ps, "ymm", "ymm", Y)
G2C(vcvtpd2dq, vcvtpd2dq, vcvtpd2dqx, "xmm", "xmm", X) G2C(vcvtpd2dq, vcvtpd2dq, vcvtpd2dqy, "ymm", "xmm", Y)
G2C(vcvttpd2dq, vcvttpd2dq, vcvttpd2dqx, "xmm", "xmm", X) G2C(vcvttpd2dq, vcvttpd2dq, vcvttpd2dqy, "ymm", "xmm", Y)
G2C(vcvtpd2ps, vcvtpd2ps, vcvtpd2psx, "xmm", "xmm", X) G2C(vcvtpd2ps, vcvtpd2ps, vcvtpd2psy, "ymm", "xmm", Y)
G2C(vcvtps2pd, vcvtps2pd, vcvtps2pd, "xmm", "xmm", X) G2C(vcvtps2pd, vcvtps2pd, vcvtps2pd, "xmm", "ymm", Y)
G2C(vcvtdq2pd, vcvtdq2pd, vcvtdq2pd, "xmm", "xmm", X) G2C(vcvtdq2pd, vcvtdq2pd, vcvtdq2pd, "xmm", "ymm", Y)
G3R(vcvtss2sd, "xmm", X) G3R(vcvtsd2ss, "xmm", X)
#define CGPRK(NAME, BODY) KRN(NAME, BODY, "mov %%rax,32(%1)")
#define F2I(M, SRC, B, REG) \
    CGPRK(k_##M##_##B##dd, #M " %%xmm2,%%" REG) CGPRK(k_##M##_##B##da, #M " %%xmm1,%%" REG) CGPRK(k_##M##_##B##mm, #M " 64(%0),%%" REG)
F2I(vcvtss2si, 4, 32, "eax") F2I(vcvtss2si, 4, 64, "rax") F2I(vcvttss2si, 4, 32, "eax") F2I(vcvttss2si, 4, 64, "rax")
F2I(vcvtsd2si, 8, 32, "eax") F2I(vcvtsd2si, 8, 64, "rax") F2I(vcvttsd2si, 8, 32, "eax") F2I(vcvttsd2si, 8, 64, "rax")
#define I2F(M, B, REG, LD, SUF) \
    KRN(k_##M##_##B##dd, LD " 64(%0),%%" REG "\n " #M " %%" REG ",%%xmm1,%%xmm0", ST0) KRN(k_##M##_##B##da, LD " 64(%0),%%" REG "\n " #M " %%" REG ",%%xmm1,%%xmm1", ST1) KRN(k_##M##_##B##mm, #M SUF " 64(%0),%%xmm1,%%xmm0", ST0)
I2F(vcvtsi2ss, 32, "eax", "mov", "l") I2F(vcvtsi2ss, 64, "rax", "mov", "q") I2F(vcvtsi2sd, 32, "eax", "mov", "l") I2F(vcvtsi2sd, 64, "rax", "mov", "q")
// ---- generator hook ----
static void fix_cvt(IRec *r, const Op *o, int stream) {
    if (stream == 6) return;
    int pv = (int)(intptr_t)o->p, kind = pv & 0xFF, bits = (pv >> 16) & 0xFF, dbl = (pv >> 24) & 1;
    int es = o->ek == EK_F64 ? 8 : o->ek == EK_F32 ? 4 : 0; if (!es) return;
    int fb = es == 8 ? 52 : 23, bias = es == 8 ? 1023 : 127, lo, hi;
    switch (kind) { case CV_PD2PS: case CV_F2F_S: lo = -160; hi = 132; break; case CV_F2I_S: case CV_F2I_V: case CV_PD2I: lo = -3; hi = bits == 64 ? 66 : 34; if (kind != CV_F2I_S) hi = 34; break; default: return; }
    if (kind == CV_F2F_S && !dbl) return;
    uint8_t *p[2] = {r->a, r->b};
    for (int q = 0; q < 2; q++) for (int i = 0; i < 32 / es; i++) {
        if (rnd() % 3 == 0) continue;
        uint64_t m = rnd() & ((1ull << fb) - 1); int k = lo + (int)(rnd() % (unsigned)(hi - lo + 1)); uint64_t s = rnd() & 1; uint64_t E = (uint64_t)(bias + k);
        int nb = kind == CV_PD2PS || kind == CV_F2F_S ? 29 : (k >= 0 && k < fb ? fb - k : 0);       // the mantissa bits below the destination's resolution
        if (nb > 0 && nb <= fb) { uint64_t low = (1ull << nb) - 1; switch (rnd() % 6) { case 0: m &= ~low; break; case 1: m = (m & ~low) | (1ull << (nb - 1)); break; case 2: m = (m & ~low) | 1; break; case 3: m |= low; break; case 4: m = (m & ~low) | (1ull << (nb - 1)) | 1; break; default: break; } }
        if (k > 0 && rnd() % 8 == 0) m = (rnd() & 1) ? 0 : ((1ull << fb) - 1);
        st_(p[q] + i * es, es, (s << (es * 8 - 1)) | (E << fb) | m);
    }
}
#define CVP(kind, trunc, bits, dbl) ((void *)(intptr_t)((kind) | ((trunc) << 8) | ((bits) << 16) | ((dbl) << 24)))
static void cv_gpr(int from) { for (int i = from; i < g_nops; i++) { g_ops[i].cr = 0; g_ops[i].xm[0] = ~0ull; } }
static void cv_fix(int from) { for (int i = from; i < g_nops; i++) { g_ops[i].fix = fix_cvt; g_ops[i].flt = 1; } }
#define CRV(M, X, Y, KIND, TR, EKK, SZX, SZY) { int f0 = g_nops; reg2(#M "_xmm", k_##M##_Xdd, k_##M##_Xda, k_##M##_Xmm, 16, EKK, m_cvt, CVP(KIND, TR, 0, 0), 1, SZX); reg2(#M "_ymm", k_##M##_Ydd, k_##M##_Yda, k_##M##_Ymm, 32, EKK, m_cvt, CVP(KIND, TR, 0, 0), 1, SZY); cv_fix(f0); }
#define CRSS(M, B, KIND, TR, DBL, EKK, SZ) { int f0 = g_nops; reg2(#M "_r" #B, k_##M##_##B##dd, k_##M##_##B##da, k_##M##_##B##mm, 16, EKK, m_cvt, CVP(KIND, TR, B, DBL), 1, SZ); cv_gpr(f0); cv_fix(f0); }
#define CRIS(M, B, DBL) { int f0 = g_nops; reg3(#M "_r" #B, k_##M##_##B##dd, k_##M##_##B##da, NULL, NULL, NULL, k_##M##_##B##mm, 16, B == 64 ? EK_I64 : EK_I32, m_cvt, CVP(CV_I2F_S, 0, B, DBL), 1, 2, 0); for (int i = f0; i < g_nops; i++) g_ops[i].sz = B / 8; cv_fix(f0); }
static void reg_convert_ops(void) {
    CRV(vcvtps2dq, 0, 0, CV_F2I_V, 0, EK_F32, 16, 32) CRV(vcvttps2dq, 0, 0, CV_F2I_V, 1, EK_F32, 16, 32) CRV(vcvtdq2ps, 0, 0, CV_I2F_V, 0, EK_I32, 16, 32)
    CRV(vcvtpd2dq, 0, 0, CV_PD2I, 0, EK_F64, 16, 32) CRV(vcvttpd2dq, 0, 0, CV_PD2I, 1, EK_F64, 16, 32) CRV(vcvtpd2ps, 0, 0, CV_PD2PS, 0, EK_F64, 16, 32)
    CRV(vcvtps2pd, 0, 0, CV_PS2PD, 0, EK_F32, 8, 16) CRV(vcvtdq2pd, 0, 0, CV_DQ2PD, 0, EK_I32, 8, 16)
    { int f0 = g_nops; reg3("vcvtss2sd_xmm", k_vcvtss2sd_Xdd, k_vcvtss2sd_Xda, k_vcvtss2sd_Xdb, k_vcvtss2sd_Xab, k_vcvtss2sd_Xal, k_vcvtss2sd_Xmm, 16, EK_F32, m_cvt, CVP(CV_F2F_S, 0, 0, 0), 1, 2, 0); for (int i = f0; i < g_nops; i++) g_ops[i].sz = 4; cv_fix(f0); }
    { int f0 = g_nops; reg3("vcvtsd2ss_xmm", k_vcvtsd2ss_Xdd, k_vcvtsd2ss_Xda, k_vcvtsd2ss_Xdb, k_vcvtsd2ss_Xab, k_vcvtsd2ss_Xal, k_vcvtsd2ss_Xmm, 16, EK_F64, m_cvt, CVP(CV_F2F_S, 0, 0, 1), 1, 2, 0); for (int i = f0; i < g_nops; i++) g_ops[i].sz = 8; cv_fix(f0); }
    CRSS(vcvtss2si, 32, CV_F2I_S, 0, 0, EK_F32, 4) CRSS(vcvtss2si, 64, CV_F2I_S, 0, 0, EK_F32, 4) CRSS(vcvttss2si, 32, CV_F2I_S, 1, 0, EK_F32, 4) CRSS(vcvttss2si, 64, CV_F2I_S, 1, 0, EK_F32, 4)
    CRSS(vcvtsd2si, 32, CV_F2I_S, 0, 1, EK_F64, 8) CRSS(vcvtsd2si, 64, CV_F2I_S, 0, 1, EK_F64, 8) CRSS(vcvttsd2si, 32, CV_F2I_S, 1, 1, EK_F64, 8) CRSS(vcvttsd2si, 64, CV_F2I_S, 1, 1, EK_F64, 8)
    CRIS(vcvtsi2ss, 32, 0) CRIS(vcvtsi2ss, 64, 0) CRIS(vcvtsi2sd, 32, 1) CRIS(vcvtsi2sd, 64, 1)
}
static int plan_cv(const Op *o) {
    int fl = 0; int dd = o->alias == AL_DD; long n = dd ? (4L << 20) : (1L << 20), m = dd ? (1L << 20) : (1L << 18);
    fl += run_stream_op(o, 0x1F80, n, 0, NULL);
    for (int rc = 1; rc < 4; rc++) fl += run_stream_op(o, 0x1F80 | (rc << 13), m, 0, NULL);
    fl += run_stream_op(o, 0x1F80 | 0x40, m, 0, NULL); fl += run_stream_op(o, 0x1F80 | 0x8000, m, 0, NULL); fl += run_stream_op(o, 0x1F80 | 0x8000 | 0x40, m, 0, NULL);
    if (dd) { fl += run_stream_op(o, 0x1F80 | 0x8000 | 0x40 | (1 << 13), m, 0, NULL); fl += run_stream_op(o, 0x1F80 | 0x8000 | (2 << 13), m, 0, NULL); }
    if (o->alias == AL_MEM && o->al == 0) fl += run_straddle(o);
    return fl;
}
#endif
