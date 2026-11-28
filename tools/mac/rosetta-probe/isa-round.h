// Family avx_round: vroundps/vroundpd/vroundss/vroundsd for all 256 immediates (rounding control bits 0-1, "use MXCSR" bit 2, precision-exception suppress bit 3, the upper four bits ignored),
// xmm and ymm, register / destination-alias / memory forms, under every MXCSR rounding mode and DAZ, bit for bit against an integer-only model.
#ifndef ISA_ROUND_H
#define ISA_ROUND_H
#include "isa-model-round.h"
STUB(vroundps_xdd, "vroundps $\\I,%xmm2,%xmm0", S_ST0)
STUB(vroundps_xda, "vroundps $\\I,%xmm1,%xmm1", S_ST1)
STUB(vroundps_xmm, "vroundps $\\I,64(%rcx),%xmm0", S_ST0)
STUB(vroundps_ydd, "vroundps $\\I,%ymm2,%ymm0", S_ST0)
STUB(vroundps_yda, "vroundps $\\I,%ymm1,%ymm1", S_ST1)
STUB(vroundps_ymm, "vroundps $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vroundpd_xdd, "vroundpd $\\I,%xmm2,%xmm0", S_ST0)
STUB(vroundpd_xda, "vroundpd $\\I,%xmm1,%xmm1", S_ST1)
STUB(vroundpd_xmm, "vroundpd $\\I,64(%rcx),%xmm0", S_ST0)
STUB(vroundpd_ydd, "vroundpd $\\I,%ymm2,%ymm0", S_ST0)
STUB(vroundpd_yda, "vroundpd $\\I,%ymm1,%ymm1", S_ST1)
STUB(vroundpd_ymm, "vroundpd $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vroundss_xdd, "vroundss $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vroundss_xda, "vroundss $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vroundss_xmm, "vroundss $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vroundsd_xdd, "vroundsd $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vroundsd_xda, "vroundsd $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vroundsd_xmm, "vroundsd $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
// op->p: dbl | scalar << 1
static void m_round(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, dbl = pv & 1, sc = (pv >> 1) & 1, es = dbl ? 8 : 4, al = c->op->alias; Fmt f = dbl ? F64 : F32; Fpc fc = fpc_of(c->mx); memset(o->r, 0, 32);
    const uint8_t *src = al == AL_MEM ? msrc(r, c->op) : (!sc && al == AL_DA) ? r->a : r->b;
    if (sc) { memcpy(o->r, r->a, 16); se_(o->r, 0, es, rnd_int(f, el_(src, 0, es), c->imm, fc)); return; }
    for (int i = 0; i < c->w / es; i++) se_(o->r, i, es, rnd_int(f, el_(src, i, es), c->imm, fc));
}
static Op *rrd(const char *name, const char *tab, int dbl, int sc, int w, int alias, int sz) {
    Op *o = newop(name, NULL, m_round, w, dbl ? EK_F64 : EK_F32); o->kimm = tab; o->p = (void *)(intptr_t)(dbl | (sc << 1)); o->alias = alias; o->nsrc = sc ? 2 : 1; o->flt = 1; o->sz = sz; return o; }
static void fix_round(IRec *r, const Op *o, int stream) {
    if (stream == 6) return; int dbl = (int)(intptr_t)o->p & 1, es = dbl ? 8 : 4, fb = dbl ? 52 : 23, bias = dbl ? 1023 : 127; uint8_t *p[2] = {r->a, r->b};
    for (int q = 0; q < 2; q++) for (int i = 0; i < 32 / es; i++) {
        if (rnd() % 3 == 0) continue; uint64_t m = rnd() & ((1ull << fb) - 1), s = rnd() & 1; int k = (int)(rnd() % (unsigned)(fb + 6)) - 4; uint64_t E = (uint64_t)(bias + k);
        int nb = k >= 0 && k < fb ? fb - k : fb; uint64_t low = (1ull << nb) - 1;
        switch (rnd() % 6) { case 0: m &= ~low; break; case 1: m = (m & ~low) | (1ull << (nb - 1)); break; case 2: m = (m & ~low) | 1; break; case 3: m |= low; break; case 4: m = (m & ~low) | (1ull << (nb - 1)) | 1; break; default: break; }
        st_(p[q] + i * es, es, (s << (es * 8 - 1)) | (E << fb) | m); }
}
static void reg_round_ops(void) {
    int f0 = g_nops;
#define RRV(M, DBL) \
    rrd(#M "_xmm", isa_tab_##M##_xdd, DBL, 0, 16, AL_DD, 0); rrd(#M "_xmm_da", isa_tab_##M##_xda, DBL, 0, 16, AL_DA, 0); rrd(#M "_xmm_mem", isa_tab_##M##_xmm, DBL, 0, 16, AL_MEM, 16); \
    rrd(#M "_ymm", isa_tab_##M##_ydd, DBL, 0, 32, AL_DD, 0); rrd(#M "_ymm_da", isa_tab_##M##_yda, DBL, 0, 32, AL_DA, 0); rrd(#M "_ymm_mem", isa_tab_##M##_ymm, DBL, 0, 32, AL_MEM, 32);
#define RRS(M, DBL) rrd(#M "_xmm", isa_tab_##M##_xdd, DBL, 1, 16, AL_DD, 0); rrd(#M "_xmm_da", isa_tab_##M##_xda, DBL, 1, 16, AL_DA, 0); rrd(#M "_xmm_mem", isa_tab_##M##_xmm, DBL, 1, 16, AL_MEM, DBL ? 8 : 4);
    RRV(vroundps, 0) RRV(vroundpd, 1) RRS(vroundss, 0) RRS(vroundsd, 1)
    for (int i = f0; i < g_nops; i++) g_ops[i].fix = fix_round;
}
static int plan_rnd(const Op *o) {
    int fl = 0; int dd = o->alias == AL_DD; long n = dd ? (2L << 20) : (1L << 20), m = dd ? (1L << 20) : (1L << 19);
    fl += run_stream_op(o, 0x1F80, n, 0, NULL);
    for (int rc = 1; rc < 4; rc++) fl += run_stream_op(o, 0x1F80 | (rc << 13), m, 0, NULL);
    fl += run_stream_op(o, 0x1F80 | 0x40, m, 0, NULL); fl += run_stream_op(o, 0x1F80 | 0x40 | (1 << 13), m, 0, NULL); fl += run_stream_op(o, 0x1F80 | 0x8000, m / 2, 0, NULL);
    if (o->alias == AL_MEM && o->al == 0) fl += run_straddle(o);
    return fl;
}
#endif
