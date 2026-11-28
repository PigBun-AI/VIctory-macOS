// Family avx_masked: vmaskmovps/pd and vpmaskmovd/q, load and store, xmm and ymm: a lane moves when the top bit of the mask lane is set; a masked-out load lane is 0 and a
// masked-out store lane leaves memory unchanged. Memory operands at offset 0 and at an odd offset, across 4 KiB / 16 KiB page boundaries. (Faulting masked-out lanes are the subject of
// the earlier fault-continuation proof, not of this probe.)
#ifndef ISA_MASKED_H
#define ISA_MASKED_H
#include "isa-model-masked.h"
#define MML(M, W, R, OFF) KRN(k_##M##_##W##l##OFF, #M " " #OFF "+64(%0),%%" R "1,%%" R "0", ST0)
#define MMS(M, W, R, OFF) KST(k_##M##_##W##s##OFF, #M " %%" R "2,%%" R "1," #OFF "(%1)")
#define MMALL(M) MML(M, X, "xmm", 0) MML(M, X, "xmm", 3) MML(M, Y, "ymm", 0) MML(M, Y, "ymm", 3) MMS(M, X, "xmm", 0) MMS(M, X, "xmm", 3) MMS(M, Y, "ymm", 0)
MMALL(vmaskmovps) MMALL(vmaskmovpd) MMALL(vpmaskmovd) MMALL(vpmaskmovq)
static void reg_masked_ops(void) {
    struct { const char *n; int es; } v[] = {{"vmaskmovps", 4}, {"vmaskmovpd", 8}, {"vpmaskmovd", 4}, {"vpmaskmovq", 8}};
    static KF ld[4][4], stt[4][3];
#define SETK(I, M) ld[I][0] = k_##M##_Xl0; ld[I][1] = k_##M##_Xl3; ld[I][2] = k_##M##_Yl0; ld[I][3] = k_##M##_Yl3; stt[I][0] = k_##M##_Xs0; stt[I][1] = k_##M##_Xs3; stt[I][2] = k_##M##_Ys0;
    SETK(0, vmaskmovps) SETK(1, vmaskmovpd) SETK(2, vpmaskmovd) SETK(3, vpmaskmovq)
    for (int i = 0; i < 4; i++) {
        const char *w[4] = {"xmm", "xmm", "ymm", "ymm"}; int off[4] = {0, 3, 0, 3};
        for (int j = 0; j < 4; j++) { char b[64]; snprintf(b, sizeof b, "%s_%s_load_o%d", v[i].n, w[j], off[j]); Op *o = newop(nm_(b, ""), ld[i][j], m_mask, j < 2 ? 16 : 32, v[i].es == 4 ? EK_I32 : EK_I64); o->p = (void *)(intptr_t)v[i].es; o->alias = AL_MEM; o->memoff = off[j]; o->sz = o->w; o->nsrc = 2; }
        const char *sw[3] = {"xmm", "xmm", "ymm"}; int so[3] = {0, 3, 0};
        for (int j = 0; j < 3; j++) { char b[64]; snprintf(b, sizeof b, "%s_%s_store_o%d", v[i].n, sw[j], so[j]); Op *o = newop(nm_(b, ""), stt[i][j], m_mask, j < 2 ? 16 : 32, v[i].es == 4 ? EK_I32 : EK_I64); o->p = (void *)(intptr_t)(v[i].es | (1 << 8)); o->mem_r = 1; o->memoff = so[j]; o->sz = o->w; o->cr = 32; o->nsrc = 2; }
    }
}
static int plan_mask(const Op *o) { int fl = run_stream_op(o, 0x1F80, 4L << 20, 0, NULL); if ((o->alias == AL_MEM || o->mem_r) && o->al == 0) fl += run_straddle(o); return fl; }
#endif
