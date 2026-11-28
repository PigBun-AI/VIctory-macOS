// Family avx_compare: vcmpps/pd/ss/sd with all 32 predicates (xmm, ymm, memory source) and vucomiss/sd, vcomiss/sd flags, default MXCSR and DAZ.
#ifndef ISA_COMPARE_H
#define ISA_COMPARE_H
#include "isa-model-cmp.h"
#define P32(F, ...) F(__VA_ARGS__, 0) F(__VA_ARGS__, 1) F(__VA_ARGS__, 2) F(__VA_ARGS__, 3) F(__VA_ARGS__, 4) F(__VA_ARGS__, 5) F(__VA_ARGS__, 6) F(__VA_ARGS__, 7) \
    F(__VA_ARGS__, 8) F(__VA_ARGS__, 9) F(__VA_ARGS__, 10) F(__VA_ARGS__, 11) F(__VA_ARGS__, 12) F(__VA_ARGS__, 13) F(__VA_ARGS__, 14) F(__VA_ARGS__, 15) \
    F(__VA_ARGS__, 16) F(__VA_ARGS__, 17) F(__VA_ARGS__, 18) F(__VA_ARGS__, 19) F(__VA_ARGS__, 20) F(__VA_ARGS__, 21) F(__VA_ARGS__, 22) F(__VA_ARGS__, 23) \
    F(__VA_ARGS__, 24) F(__VA_ARGS__, 25) F(__VA_ARGS__, 26) F(__VA_ARGS__, 27) F(__VA_ARGS__, 28) F(__VA_ARGS__, 29) F(__VA_ARGS__, 30) F(__VA_ARGS__, 31)
#define CMPK(M, I) KRN(k_##M##_x##I, #M " $" #I ",%%xmm2,%%xmm1,%%xmm0", ST0) KRN(k_##M##_xa##I, #M " $" #I ",%%xmm2,%%xmm1,%%xmm1", ST1) KRN(k_##M##_xm##I, #M " $" #I ",64(%0),%%xmm1,%%xmm0", ST0) \
    KRN(k_##M##_y##I, #M " $" #I ",%%ymm2,%%ymm1,%%ymm0", ST0) KRN(k_##M##_ym##I, #M " $" #I ",64(%0),%%ymm1,%%ymm0", ST0)
#define CMPKS(M, I) KRN(k_##M##_x##I, #M " $" #I ",%%xmm2,%%xmm1,%%xmm0", ST0) KRN(k_##M##_xa##I, #M " $" #I ",%%xmm2,%%xmm1,%%xmm1", ST1) KRN(k_##M##_xm##I, #M " $" #I ",64(%0),%%xmm1,%%xmm0", ST0)
P32(CMPK, vcmpps) P32(CMPK, vcmppd) P32(CMPKS, vcmpss) P32(CMPKS, vcmpsd)
// flag compares: operands in xmm1 (first) and xmm2 / memory
#define FLK(NAME, BODY) static void NAME(const IRec *in, Out *out, long n) { \
    A("1:\n" K_PRE "\n " BODY "\n pushfq\n popq 40(%1)\n add $128,%0\n add $64,%1\n dec %2\n jnz 1b" : "+r"(in), "+r"(out), "+r"(n) :: "ymm0", "ymm1", "ymm2", "ymm3", "memory", "cc"); }
FLK(k_vucomiss_x, "vucomiss %%xmm2,%%xmm1") FLK(k_vucomiss_m, "vucomiss 64(%0),%%xmm1") FLK(k_vucomisd_x, "vucomisd %%xmm2,%%xmm1") FLK(k_vucomisd_m, "vucomisd 64(%0),%%xmm1")
FLK(k_vcomiss_x, "vcomiss %%xmm2,%%xmm1") FLK(k_vcomiss_m, "vcomiss 64(%0),%%xmm1") FLK(k_vcomisd_x, "vcomisd %%xmm2,%%xmm1") FLK(k_vcomisd_m, "vcomisd 64(%0),%%xmm1")
FLK(k_vucomiss_a, "vucomiss %%xmm1,%%xmm1") FLK(k_vucomisd_a, "vucomisd %%xmm1,%%xmm1") FLK(k_vcomiss_a, "vcomiss %%xmm1,%%xmm1") FLK(k_vcomisd_a, "vcomisd %%xmm1,%%xmm1")
static void um_cmpflags(const IRec *r, const Op *o, uint64_t m[4]) { (void)r; (void)o; m[0] = m[1] = m[2] = m[3] = 0; }
static void reg_cmp_pred(const char *base, int I, KF kx, KF kxa, KF kxm, KF ky, KF kym, int dbl, int sc) {
    char b[80]; Op *o; int ek = dbl ? EK_F64 : EK_F32; void *p = (void *)(intptr_t)(I | (dbl << 8) | (sc << 16));
    snprintf(b, sizeof b, "%s_p%d_xmm", base, I); o = newop(nm_(b, ""), kx, m_vcmp, 16, ek); o->p = p; o->flt = 1; o->nsrc = 2;
    snprintf(b, sizeof b, "%s_p%d_xmm_da", base, I); o = newop(nm_(b, ""), kxa, m_vcmp, 16, ek); o->p = p; o->flt = 1; o->nsrc = 2; o->alias = AL_DA;
    snprintf(b, sizeof b, "%s_p%d_xmm_mem", base, I); o = newop(nm_(b, ""), kxm, m_vcmp, 16, ek); o->p = p; o->flt = 1; o->nsrc = 2; o->alias = AL_MEM; o->sz = sc ? (dbl ? 8 : 4) : 16;
    if (ky) { snprintf(b, sizeof b, "%s_p%d_ymm", base, I); o = newop(nm_(b, ""), ky, m_vcmp, 32, ek); o->p = p; o->flt = 1; o->nsrc = 2;
              snprintf(b, sizeof b, "%s_p%d_ymm_mem", base, I); o = newop(nm_(b, ""), kym, m_vcmp, 32, ek); o->p = p; o->flt = 1; o->nsrc = 2; o->alias = AL_MEM; o->sz = 32; }
}
#define RCMP(M, DBL, I) reg_cmp_pred(#M, I, k_##M##_x##I, k_##M##_xa##I, k_##M##_xm##I, k_##M##_y##I, k_##M##_ym##I, DBL, 0);
#define RCMS(M, DBL, I) reg_cmp_pred(#M, I, k_##M##_x##I, k_##M##_xa##I, k_##M##_xm##I, NULL, NULL, DBL, 1);
#define RCMPP(M, DBL) P32(RCMP, M, DBL)
#define RCMPS(M, DBL) P32(RCMS, M, DBL)
static void reg_compare_ops(void) {
    Op *o; char b[64];
    RCMPP(vcmpps, 0) RCMPP(vcmppd, 1) RCMPS(vcmpss, 0) RCMPS(vcmpsd, 1)
    struct { const char *n; KF k; int dbl, al; } f[] = {{"vucomiss_x_x", k_vucomiss_x, 0, AL_DD}, {"vucomiss_x_m32", k_vucomiss_m, 0, AL_MEM}, {"vucomisd_x_x", k_vucomisd_x, 1, AL_DD}, {"vucomisd_x_m64", k_vucomisd_m, 1, AL_MEM},
        {"vcomiss_x_x", k_vcomiss_x, 0, AL_DD}, {"vcomiss_x_m32", k_vcomiss_m, 0, AL_MEM}, {"vcomisd_x_x", k_vcomisd_x, 1, AL_DD}, {"vcomisd_x_m64", k_vcomisd_m, 1, AL_MEM},
        {"vucomiss_x_same", k_vucomiss_a, 0, AL_AB}, {"vucomisd_x_same", k_vucomisd_a, 1, AL_AB}, {"vcomiss_x_same", k_vcomiss_a, 0, AL_AB}, {"vcomisd_x_same", k_vcomisd_a, 1, AL_AB}};
    for (unsigned i = 0; i < sizeof f / sizeof *f; i++) { o = newop(f[i].n, f[i].k, m_ucomi, 16, f[i].dbl ? EK_F64 : EK_F32); o->p = (void *)(intptr_t)f[i].dbl; o->flt = 1; o->nsrc = 2; o->alias = f[i].al; o->sz = f[i].dbl ? 8 : 4;
        o->cr = 0; o->xm[1] = FL_ALL; (void)b; }
}
static int plan_cmp(const Op *o) {
    int fl = 0; int dd = o->alias == AL_DD; long n = dd ? (2L << 20) : (1L << 20);
    fl += run_stream_op(o, 0x1F80, n, 0, NULL);
    fl += run_stream_op(o, 0x1F80 | 0x40, n / 2, 0, NULL);
    if (o->alias == AL_MEM && o->al == 0 && (o->sz != 16 || (((int)(intptr_t)o->p & 0xFF) < 4))) fl += run_straddle(o);
    return fl;
}
#endif
