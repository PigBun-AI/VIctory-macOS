// Family avx_fparith: vadd/vsub/vmul/vdiv/vmax/vmin (ps, pd, ss, sd; xmm and ymm; register, memory and alias forms) and vsqrt (ps, pd, ss, sd),
// against the integer software model in isa-model-fp.h, under every MXCSR rounding mode, DAZ, FTZ and DAZ+FTZ.
#ifndef ISA_FPARITH_H
#define ISA_FPARITH_H
#include "isa-model-fp.h"
#define FP6(P) G3(P##ps) G3(P##pd) G3R(P##ss, "xmm", X) G3R(P##sd, "xmm", X)
FP6(vadd) FP6(vsub) FP6(vmul) FP6(vdiv) FP6(vmax) FP6(vmin)
G3R(vsqrtss, "xmm", X) G3R(vsqrtsd, "xmm", X)
#define SQ2(M) G2R(M, "xmm", "xmm", X) G2R(M, "ymm", "ymm", Y)
SQ2(vsqrtps) SQ2(vsqrtpd)
#define REG3S(M, KIND, DBL) reg3(#M "_xmm", k_##M##_Xdd, k_##M##_Xda, k_##M##_Xdb, k_##M##_Xab, k_##M##_Xal, k_##M##_Xmm, 16, DBL ? EK_F64 : EK_F32, m_fparith, (void *)(intptr_t)(KIND | (DBL << 8) | (1 << 16)), 1, 2, 0);
#define REG3P(M, KIND, DBL) REG3(M, m_fparith, DBL ? EK_F64 : EK_F32, (void *)(intptr_t)(KIND | (DBL << 8)), 1, 2, 0)
#define REGFP6(P, KIND) REG3P(P##ps, KIND, 0) REG3P(P##pd, KIND, 1) REG3S(P##ss, KIND, 0) REG3S(P##sd, KIND, 1)
static void reg_fparith_ops(void) {
    REGFP6(vadd, FP_ADD) REGFP6(vsub, FP_SUB) REGFP6(vmul, FP_MUL) REGFP6(vdiv, FP_DIV) REGFP6(vmax, FP_MAX) REGFP6(vmin, FP_MIN)
    REG3S(vsqrtss, FP_SQRT, 0) REG3S(vsqrtsd, FP_SQRT, 1)
    {
      reg2("vsqrtps_xmm", k_vsqrtps_Xdd, k_vsqrtps_Xda, k_vsqrtps_Xmm, 16, EK_F32, m_fparith, (void *)(intptr_t)FP_SQRT, 1, 0);
      reg2("vsqrtps_ymm", k_vsqrtps_Ydd, k_vsqrtps_Yda, k_vsqrtps_Ymm, 32, EK_F32, m_fparith, (void *)(intptr_t)FP_SQRT, 1, 0);
      reg2("vsqrtpd_xmm", k_vsqrtpd_Xdd, k_vsqrtpd_Xda, k_vsqrtpd_Xmm, 16, EK_F64, m_fparith, (void *)((intptr_t)FP_SQRT | 256), 1, 0);
      reg2("vsqrtpd_ymm", k_vsqrtpd_Ydd, k_vsqrtpd_Yda, k_vsqrtpd_Ymm, 32, EK_F64, m_fparith, (void *)((intptr_t)FP_SQRT | 256), 1, 0); }
}
// MXCSR plan: the default mode in full; the other three rounding modes, DAZ, FTZ and DAZ+FTZ on the distinct-register form (and the alias forms with one of each)
static int plan_fp(const Op *o) {
    int fl = 0; int dd = o->alias == AL_DD; long n = dd ? (8L << 20) : (2L << 20), m = dd ? (2L << 20) : (1L << 19);
    fl += run_stream_op(o, 0x1F80, n, o->exh, NULL);
    for (int rc = 1; rc < 4; rc++) if (dd || rc == 2) fl += run_stream_op(o, 0x1F80 | (rc << 13), m, 0, NULL);
    fl += run_stream_op(o, 0x1F80 | 0x40, m, 0, NULL);
    fl += run_stream_op(o, 0x1F80 | 0x8000, m, 0, NULL);
    fl += run_stream_op(o, 0x1F80 | 0x8000 | 0x40, m, 0, NULL);
    if (dd) { fl += run_stream_op(o, 0x1F80 | 0x8000 | 0x40 | (1 << 13), m, 0, NULL); fl += run_stream_op(o, 0x1F80 | 0x8000 | (2 << 13), m, 0, NULL); }
    if (o->alias == AL_MEM && o->al == 0) fl += run_straddle(o);
    return fl;
}
#endif
