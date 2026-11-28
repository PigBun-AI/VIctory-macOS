// Family avx_move: AVX moves, broadcasts and mask extraction (vmovaps/ups/apd/upd/dqa/dqu, vmovss/sd, vmovd/q, vmovlps/hps/lpd/hpd,
// vmovlhps/hlps, vmovshdup/sldup/ddup, vbroadcastss/sd/f128/i128, vpbroadcastb/w/d/q, vmovmskps/pd, vpmovmskb, vlddqu) in every operand
// form the title uses: register<-register (distinct and dst==src), register<-memory (aligned and unaligned offsets, and a source straddling a
// 4 KiB / 16 KiB boundary), memory<-register (stores to aligned/unaligned offsets), GPR<->xmm (four different GPRs).
// Models are byte moves (no arithmetic); expectations include VEX zeroing of bits above the vector length and the zero-extension of 32-bit GPR results.
#ifndef ISA_MOVE_H
#define ISA_MOVE_H

#include "isa-model-move.h"

// ---- kernels ----
// register <- register, register <- memory (offsets MO = 64 + n), memory <- register
#define MV_RR(M) G2R(M, "xmm", "xmm", X) G2R(M, "ymm", "ymm", Y)
#define MV_LD(M, R, W, MO) KRN(k_##M##_##W##l##MO, #M " " #MO "(%0),%%" R "0", ST0)
#define MV_ST(M, R, W, OFF) KST(k_##M##_##W##s##OFF, #M " %%" R "1," #OFF "(%1)")
#define MV_FULL(M) MV_RR(M) \
    MV_LD(M, "xmm", X, 65) MV_LD(M, "xmm", X, 67) MV_LD(M, "xmm", X, 79) MV_LD(M, "xmm", X, 81) MV_LD(M, "xmm", X, 95) \
    MV_LD(M, "ymm", Y, 65) MV_LD(M, "ymm", Y, 67) MV_LD(M, "ymm", Y, 79) MV_LD(M, "ymm", Y, 81) MV_LD(M, "ymm", Y, 95) \
    MV_ST(M, "xmm", X, 1) MV_ST(M, "xmm", X, 3) MV_ST(M, "xmm", X, 15) MV_ST(M, "xmm", X, 17) MV_ST(M, "xmm", X, 31) \
    MV_ST(M, "ymm", Y, 1) MV_ST(M, "ymm", Y, 3) MV_ST(M, "ymm", Y, 15) MV_ST(M, "ymm", Y, 17) MV_ST(M, "ymm", Y, 31) \
    MV_LD(M, "xmm", X, 64) MV_LD(M, "xmm", X, 80) MV_LD(M, "ymm", Y, 64) MV_ST(M, "xmm", X, 0) MV_ST(M, "xmm", X, 16) MV_ST(M, "ymm", Y, 0)
MV_FULL(vmovaps) MV_FULL(vmovups) MV_FULL(vmovapd) MV_FULL(vmovupd) MV_FULL(vmovdqa) MV_FULL(vmovdqu)
// vlddqu: unaligned load only
#define LDDQU(R, W, MO) KRN(k_vlddqu_##W##l##MO, "vlddqu " #MO "(%0),%%" R "0", ST0)
LDDQU("xmm", X, 64) LDDQU("xmm", X, 65) LDDQU("xmm", X, 79) LDDQU("xmm", X, 81) LDDQU("ymm", Y, 64) LDDQU("ymm", Y, 65) LDDQU("ymm", Y, 79) LDDQU("ymm", Y, 95)

static void reg_mv_full(const char *M, int aligned, KF xdd, KF xda, KF ydd, KF yda, KF *xl, KF *yl, KF *xs, KF *ys) {
    static const int ldoff[5] = {1, 3, 15, 17, 31}; static const int stoff[5] = {1, 3, 15, 17, 31};
    char b[64]; Op *o;
    snprintf(b, sizeof b, "%s_xmm", M); reg2(nm_(b, ""), xdd, xda, NULL, 16, EK_I32, m_copy, NULL, 0, 0);
    snprintf(b, sizeof b, "%s_ymm", M); reg2(nm_(b, ""), ydd, yda, NULL, 32, EK_I32, m_copy, NULL, 0, 0);
    for (int w = 0; w < 2; w++) {
        KF *ls = w ? yl : xl, *ss = w ? ys : xs; int vw = w ? 32 : 16; const char *wn = w ? "ymm" : "xmm";
        // ls[0] = offset 0 load, ls[1] = 16 (xmm only), ls[2..6] = unaligned; ss likewise
        for (int i = 0; i < 7; i++) {
            if (!ls[i]) continue; int off = i == 0 ? 0 : (i == 1 ? 16 : ldoff[i - 2]);
            if (aligned && off % vw) continue;
            snprintf(b, sizeof b, "%s_%s_load_o%d", M, wn, off); o = newop(nm_(b, ""), ls[i], m_load, vw, EK_I32); o->alias = AL_MEM; o->memoff = off; o->nsrc = 1; o->sz = vw; o->al = aligned ? vw : 0;
        }
        for (int i = 0; i < 7; i++) {
            if (!ss[i]) continue; int off = i == 0 ? 0 : (i == 1 ? 16 : stoff[i - 2]);
            if (aligned && off % vw) continue;
            snprintf(b, sizeof b, "%s_%s_store_o%d", M, wn, off); o = newop(nm_(b, ""), ss[i], m_store, vw, EK_I32); o->mem_image = 1; o->memoff = off; o->cr = 0; o->nsrc = 1; o->sz = vw; o->al = aligned ? vw : 0;
            o->xm[0] = o->xm[1] = o->xm[2] = o->xm[3] = ~0ull;
        }
    }
}
#define REGMV(M, ALIGNED) { \
    KF xl[7] = {k_##M##_Xl64, k_##M##_Xl80, k_##M##_Xl65, k_##M##_Xl67, k_##M##_Xl79, k_##M##_Xl81, k_##M##_Xl95}; \
    KF yl[7] = {k_##M##_Yl64, NULL, k_##M##_Yl65, k_##M##_Yl67, k_##M##_Yl79, k_##M##_Yl81, k_##M##_Yl95}; \
    KF xs[7] = {k_##M##_Xs0, k_##M##_Xs16, k_##M##_Xs1, k_##M##_Xs3, k_##M##_Xs15, k_##M##_Xs17, k_##M##_Xs31}; \
    KF ys[7] = {k_##M##_Ys0, NULL, k_##M##_Ys1, k_##M##_Ys3, k_##M##_Ys15, k_##M##_Ys17, k_##M##_Ys31}; \
    reg_mv_full(#M, ALIGNED, k_##M##_Xdd, k_##M##_Xda, k_##M##_Ydd, k_##M##_Yda, xl, yl, xs, ys); }

// vmovss / vmovsd: x,x,x (6 alias variants), x,m (load, zeroes the rest), m,x (store)
G3R(vmovss, "xmm", X) G3R(vmovsd, "xmm", X)
KRN(k_vmovss_ld0, "vmovss 64(%0),%%xmm0", ST0) KRN(k_vmovss_ld1, "vmovss 65(%0),%%xmm0", ST0) KRN(k_vmovss_ld3, "vmovss 67(%0),%%xmm0", ST0) KRN(k_vmovss_ld13, "vmovss 77(%0),%%xmm0", ST0)
KRN(k_vmovsd_ld0, "vmovsd 64(%0),%%xmm0", ST0) KRN(k_vmovsd_ld1, "vmovsd 65(%0),%%xmm0", ST0) KRN(k_vmovsd_ld7, "vmovsd 71(%0),%%xmm0", ST0) KRN(k_vmovsd_ld13, "vmovsd 77(%0),%%xmm0", ST0)
KST(k_vmovss_st0, "vmovss %%xmm1,0(%1)") KST(k_vmovss_st1, "vmovss %%xmm1,1(%1)") KST(k_vmovss_st3, "vmovss %%xmm1,3(%1)") KST(k_vmovss_st30, "vmovss %%xmm1,30(%1)") KST(k_vmovss_st60, "vmovss %%xmm1,60(%1)")
KST(k_vmovsd_st0, "vmovsd %%xmm1,0(%1)") KST(k_vmovsd_st1, "vmovsd %%xmm1,1(%1)") KST(k_vmovsd_st7, "vmovsd %%xmm1,7(%1)") KST(k_vmovsd_st30, "vmovsd %%xmm1,30(%1)") KST(k_vmovsd_st56, "vmovsd %%xmm1,56(%1)")

// vmovd / vmovq between xmm and GPR (four GPRs), vmovd/vmovq with memory, vmovq x,x
#define GPRK(NAME, BODY) static void NAME(const IRec *in, Out *out, long n) { \
    A("1:\n" K_PRE "\n " BODY "\n add $128,%0\n add $64,%1\n dec %2\n jnz 1b" : "+r"(in), "+r"(out), "+r"(n) :: "ymm0", "ymm1", "ymm2", "ymm3", "rax", "rsi", "r9", "r15", "memory", "cc"); }
#define MOVD_G(G32, G64, TAG) \
    GPRK(k_vmovd_to_##TAG, "mov $-1,%%" G64 "\n mov 64(%0),%%" G32 "\n vmovd %%" G32 ",%%xmm0\n vmovups %%ymm0,(%1)") \
    GPRK(k_vmovq_to_##TAG, "mov 64(%0),%%" G64 "\n vmovq %%" G64 ",%%xmm0\n vmovups %%ymm0,(%1)") \
    GPRK(k_vmovd_from_##TAG, "mov $-1,%%" G64 "\n vmovd %%xmm2,%%" G32 "\n mov %%" G64 ",32(%1)") \
    GPRK(k_vmovq_from_##TAG, "mov $-1,%%" G64 "\n vmovq %%xmm2,%%" G64 "\n mov %%" G64 ",32(%1)")
MOVD_G("eax", "rax", rax) MOVD_G("esi", "rsi", rsi) MOVD_G("r9d", "r9", r9) MOVD_G("r15d", "r15", r15)
KRN(k_vmovq_xx_dd, "vmovq %%xmm2,%%xmm0", ST0) KRN(k_vmovq_xx_da, "vmovq %%xmm1,%%xmm1", ST1)
KRN(k_vmovd_ld0, "vmovd 64(%0),%%xmm0", ST0) KRN(k_vmovd_ld3, "vmovd 67(%0),%%xmm0", ST0) KRN(k_vmovq_ld0, "vmovq 64(%0),%%xmm0", ST0) KRN(k_vmovq_ld7, "vmovq 71(%0),%%xmm0", ST0)
KST(k_vmovd_st0, "vmovd %%xmm1,0(%1)") KST(k_vmovd_st3, "vmovd %%xmm1,3(%1)") KST(k_vmovd_st29, "vmovd %%xmm1,29(%1)") KST(k_vmovq_st0, "vmovq %%xmm1,0(%1)") KST(k_vmovq_st5, "vmovq %%xmm1,5(%1)") KST(k_vmovq_st56, "vmovq %%xmm1,56(%1)")

// vmovlps/vmovlpd/vmovhps/vmovhpd: x,x,m64 (dst distinct / dst==src1) and m64,x stores; vmovlhps/vmovhlps x,x,x
#define LH_LD(M) KRN(k_##M##_ld0, #M " 64(%0),%%xmm1,%%xmm0", ST0) KRN(k_##M##_ld1, #M " 65(%0),%%xmm1,%%xmm0", ST0) KRN(k_##M##_ld7, #M " 71(%0),%%xmm1,%%xmm0", ST0) KRN(k_##M##_lda, #M " 64(%0),%%xmm1,%%xmm1", ST1)
LH_LD(vmovlps) LH_LD(vmovhps) LH_LD(vmovlpd) LH_LD(vmovhpd)
#define LH_ST(M) KST(k_##M##_st0, #M " %%xmm1,0(%1)") KST(k_##M##_st3, #M " %%xmm1,3(%1)") KST(k_##M##_st24, #M " %%xmm1,24(%1)") KST(k_##M##_st56, #M " %%xmm1,56(%1)")
LH_ST(vmovlps) LH_ST(vmovhps) LH_ST(vmovlpd) LH_ST(vmovhpd)
G3R(vmovlhps, "xmm", X) G3R(vmovhlps, "xmm", X)

// vmovshdup / vmovsldup / vmovddup
#define DUPK(M) G2R(M, "xmm", "xmm", X) G2R(M, "ymm", "ymm", Y)
DUPK(vmovshdup) DUPK(vmovsldup) DUPK(vmovddup)
KRN(k_vmovddup_Xm1, "vmovddup 65(%0),%%xmm0", ST0) KRN(k_vmovddup_Xm7, "vmovddup 71(%0),%%xmm0", ST0)

// broadcasts: from xmm (x,x and y,x; src==dst) and from memory (aligned and odd offsets)
#define BC(M, SZ) \
    KRN(k_##M##_Xdd, #M " %%xmm2,%%xmm0", ST0) KRN(k_##M##_Xda, #M " %%xmm1,%%xmm1", ST1) KRN(k_##M##_Ydd, #M " %%xmm2,%%ymm0", ST0) KRN(k_##M##_Yda, #M " %%xmm1,%%ymm1", ST1) \
    KRN(k_##M##_Xm0, #M " 64(%0),%%xmm0", ST0) KRN(k_##M##_Xm1, #M " 65(%0),%%xmm0", ST0) KRN(k_##M##_Xm3, #M " 67(%0),%%xmm0", ST0) KRN(k_##M##_Xm15, #M " 79(%0),%%xmm0", ST0) \
    KRN(k_##M##_Ym0, #M " 64(%0),%%ymm0", ST0) KRN(k_##M##_Ym1, #M " 65(%0),%%ymm0", ST0) KRN(k_##M##_Ym3, #M " 67(%0),%%ymm0", ST0) KRN(k_##M##_Ym15, #M " 79(%0),%%ymm0", ST0)
BC(vbroadcastss, 4) BC(vpbroadcastb, 1) BC(vpbroadcastw, 2) BC(vpbroadcastd, 4) BC(vpbroadcastq, 8)
KRN(k_vbroadcastsd_Ydd, "vbroadcastsd %%xmm2,%%ymm0", ST0) KRN(k_vbroadcastsd_Yda, "vbroadcastsd %%xmm1,%%ymm1", ST1)
KRN(k_vbroadcastsd_Ym0, "vbroadcastsd 64(%0),%%ymm0", ST0) KRN(k_vbroadcastsd_Ym1, "vbroadcastsd 65(%0),%%ymm0", ST0) KRN(k_vbroadcastsd_Ym3, "vbroadcastsd 67(%0),%%ymm0", ST0) KRN(k_vbroadcastsd_Ym15, "vbroadcastsd 79(%0),%%ymm0", ST0)
KRN(k_vbroadcastf128_Ym0, "vbroadcastf128 64(%0),%%ymm0", ST0) KRN(k_vbroadcastf128_Ym1, "vbroadcastf128 65(%0),%%ymm0", ST0) KRN(k_vbroadcastf128_Ym15, "vbroadcastf128 79(%0),%%ymm0", ST0)
KRN(k_vbroadcasti128_Ym0, "vbroadcasti128 64(%0),%%ymm0", ST0) KRN(k_vbroadcasti128_Ym1, "vbroadcasti128 65(%0),%%ymm0", ST0) KRN(k_vbroadcasti128_Ym15, "vbroadcasti128 79(%0),%%ymm0", ST0)

// mask extraction (GPR result, four GPRs)
#define MSK(M, TAG, R, G32, G64) GPRK(k_##M##_##TAG, "mov $-1,%%" G64 "\n " #M " %%" R "2,%%" G32 "\n mov %%" G64 ",32(%1)")
#define MSKALL(M) MSK(M, xrax, "xmm", "eax", "rax") MSK(M, yrax, "ymm", "eax", "rax") MSK(M, xrsi, "xmm", "esi", "rsi") MSK(M, yrsi, "ymm", "esi", "rsi") \
    MSK(M, xr9, "xmm", "r9d", "r9") MSK(M, yr9, "ymm", "r9d", "r9") MSK(M, xr15, "xmm", "r15d", "r15") MSK(M, yr15, "ymm", "r15d", "r15")
MSKALL(vmovmskps) MSKALL(vmovmskpd) MSKALL(vpmovmskb)

// ---- registration ----
static void xonly(Op *o) { o->cr = 0; o->xm[0] = ~0ull; }
static void reg_move_ops(void) {
    Op *o;
    REGMV(vmovaps, 1) REGMV(vmovups, 0) REGMV(vmovapd, 1) REGMV(vmovupd, 0) REGMV(vmovdqa, 1) REGMV(vmovdqu, 0)
    // vlddqu
    { struct { const char *n; KF k; int w, off; } v[] = {{"vlddqu_xmm_load_o0", k_vlddqu_Xl64, 16, 0}, {"vlddqu_xmm_load_o1", k_vlddqu_Xl65, 16, 1}, {"vlddqu_xmm_load_o15", k_vlddqu_Xl79, 16, 15}, {"vlddqu_xmm_load_o17", k_vlddqu_Xl81, 16, 17},
          {"vlddqu_ymm_load_o0", k_vlddqu_Yl64, 32, 0}, {"vlddqu_ymm_load_o1", k_vlddqu_Yl65, 32, 1}, {"vlddqu_ymm_load_o15", k_vlddqu_Yl79, 32, 15}, {"vlddqu_ymm_load_o31", k_vlddqu_Yl95, 32, 31}};
      for (unsigned i = 0; i < sizeof v / sizeof *v; i++) { o = newop(v[i].n, v[i].k, m_load, v[i].w, EK_I32); o->alias = AL_MEM; o->memoff = v[i].off; o->nsrc = 1; } }
    // vmovss / vmovsd
    { const char *nn[2] = {"vmovss", "vmovsd"};
      KF kk[2][6] = {{k_vmovss_Xdd, k_vmovss_Xda, k_vmovss_Xdb, k_vmovss_Xab, k_vmovss_Xal, NULL}, {k_vmovsd_Xdd, k_vmovsd_Xda, k_vmovsd_Xdb, k_vmovsd_Xab, k_vmovsd_Xal, NULL}};
      for (int i = 0; i < 2; i++) { char b[40]; snprintf(b, sizeof b, "%s_xmm_rr", nn[i]); int before = g_nops; reg3(nm_(b, ""), kk[i][0], kk[i][1], kk[i][2], kk[i][3], kk[i][4], NULL, 16, EK_I32, m_movss, NULL, 0, 2, 0); for (int j = before; j < g_nops; j++) g_ops[j].sz = i ? 8 : 4; }
      struct { const char *n; KF k; int sz, off; } l[] = {{"vmovss_xmm_load_o0", k_vmovss_ld0, 4, 0}, {"vmovss_xmm_load_o1", k_vmovss_ld1, 4, 1}, {"vmovss_xmm_load_o3", k_vmovss_ld3, 4, 3}, {"vmovss_xmm_load_o13", k_vmovss_ld13, 4, 13},
          {"vmovsd_xmm_load_o0", k_vmovsd_ld0, 8, 0}, {"vmovsd_xmm_load_o1", k_vmovsd_ld1, 8, 1}, {"vmovsd_xmm_load_o7", k_vmovsd_ld7, 8, 7}, {"vmovsd_xmm_load_o13", k_vmovsd_ld13, 8, 13}};
      for (unsigned i = 0; i < sizeof l / sizeof *l; i++) { o = newop(l[i].n, l[i].k, m_load, 16, EK_I32); o->alias = AL_MEM; o->memoff = l[i].off; o->sz = l[i].sz; o->nsrc = 1; }
      struct { const char *n; KF k; int sz, off; } s[] = {{"vmovss_store_o0", k_vmovss_st0, 4, 0}, {"vmovss_store_o1", k_vmovss_st1, 4, 1}, {"vmovss_store_o3", k_vmovss_st3, 4, 3}, {"vmovss_store_o30", k_vmovss_st30, 4, 30}, {"vmovss_store_o60", k_vmovss_st60, 4, 60},
          {"vmovsd_store_o0", k_vmovsd_st0, 8, 0}, {"vmovsd_store_o1", k_vmovsd_st1, 8, 1}, {"vmovsd_store_o7", k_vmovsd_st7, 8, 7}, {"vmovsd_store_o30", k_vmovsd_st30, 8, 30}, {"vmovsd_store_o56", k_vmovsd_st56, 8, 56}};
      for (unsigned i = 0; i < sizeof s / sizeof *s; i++) { o = newop(s[i].n, s[i].k, m_store, 16, EK_I32); o->mem_image = 1; o->memoff = s[i].off; o->sz = s[i].sz; o->cr = 0; o->nsrc = 1; for (int j = 0; j < 4; j++) o->xm[j] = ~0ull; } }
    // vmovd / vmovq GPR <-> xmm
    { static const char *gn[4] = {"rax", "rsi", "r9", "r15"};
      KF td[4] = {k_vmovd_to_rax, k_vmovd_to_rsi, k_vmovd_to_r9, k_vmovd_to_r15}, tq[4] = {k_vmovq_to_rax, k_vmovq_to_rsi, k_vmovq_to_r9, k_vmovq_to_r15};
      KF fd[4] = {k_vmovd_from_rax, k_vmovd_from_rsi, k_vmovd_from_r9, k_vmovd_from_r15}, fq[4] = {k_vmovq_from_rax, k_vmovq_from_rsi, k_vmovq_from_r9, k_vmovq_from_r15};
      for (int i = 0; i < 4; i++) { char b[48];
          snprintf(b, sizeof b, "vmovd_xmm_from_%s", gn[i]); o = newop(nm_(b, ""), td[i], m_movd_to_x, 16, EK_I32); o->sz = 4; o->nsrc = 1;
          snprintf(b, sizeof b, "vmovq_xmm_from_%s", gn[i]); o = newop(nm_(b, ""), tq[i], m_movd_to_x, 16, EK_I64); o->sz = 8; o->nsrc = 1;
          snprintf(b, sizeof b, "vmovd_%s_from_xmm", gn[i]); o = newop(nm_(b, ""), fd[i], m_movd_from_x, 16, EK_I32); o->sz = 4; o->nsrc = 1; xonly(o);
          snprintf(b, sizeof b, "vmovq_%s_from_xmm", gn[i]); o = newop(nm_(b, ""), fq[i], m_movd_from_x, 16, EK_I64); o->sz = 8; o->nsrc = 1; xonly(o); }
      o = newop("vmovq_xmm_xmm", k_vmovq_xx_dd, m_movd_to_x, 16, EK_I64); o->sz = 8; o->nsrc = 1;
      o = newop("vmovq_xmm_xmm_da", k_vmovq_xx_da, m_movd_to_x, 16, EK_I64); o->sz = 8; o->nsrc = 1; o->alias = AL_DA;
      struct { const char *n; KF k; int sz, off; } l[] = {{"vmovd_xmm_load_o0", k_vmovd_ld0, 4, 0}, {"vmovd_xmm_load_o3", k_vmovd_ld3, 4, 3}, {"vmovq_xmm_load_o0", k_vmovq_ld0, 8, 0}, {"vmovq_xmm_load_o7", k_vmovq_ld7, 8, 7}};
      for (unsigned i = 0; i < sizeof l / sizeof *l; i++) { o = newop(l[i].n, l[i].k, m_load, 16, EK_I32); o->alias = AL_MEM; o->memoff = l[i].off; o->sz = l[i].sz; o->nsrc = 1; }
      struct { const char *n; KF k; int sz, off; } s[] = {{"vmovd_store_o0", k_vmovd_st0, 4, 0}, {"vmovd_store_o3", k_vmovd_st3, 4, 3}, {"vmovd_store_o29", k_vmovd_st29, 4, 29}, {"vmovq_store_o0", k_vmovq_st0, 8, 0}, {"vmovq_store_o5", k_vmovq_st5, 8, 5}, {"vmovq_store_o56", k_vmovq_st56, 8, 56}};
      for (unsigned i = 0; i < sizeof s / sizeof *s; i++) { o = newop(s[i].n, s[i].k, m_store, 16, EK_I32); o->mem_image = 1; o->memoff = s[i].off; o->sz = s[i].sz; o->cr = 0; o->nsrc = 1; for (int j = 0; j < 4; j++) o->xm[j] = ~0ull; } }
    // vmovlps/hps/lpd/hpd, vmovlhps/hlps
    { struct { const char *n; KF ld[4], st[4]; MF m; int hi; } v[4] = {
          {"vmovlps", {k_vmovlps_ld0, k_vmovlps_ld1, k_vmovlps_ld7, k_vmovlps_lda}, {k_vmovlps_st0, k_vmovlps_st3, k_vmovlps_st24, k_vmovlps_st56}, m_lps_load, 0},
          {"vmovhps", {k_vmovhps_ld0, k_vmovhps_ld1, k_vmovhps_ld7, k_vmovhps_lda}, {k_vmovhps_st0, k_vmovhps_st3, k_vmovhps_st24, k_vmovhps_st56}, m_hps_load, 1},
          {"vmovlpd", {k_vmovlpd_ld0, k_vmovlpd_ld1, k_vmovlpd_ld7, k_vmovlpd_lda}, {k_vmovlpd_st0, k_vmovlpd_st3, k_vmovlpd_st24, k_vmovlpd_st56}, m_lps_load, 0},
          {"vmovhpd", {k_vmovhpd_ld0, k_vmovhpd_ld1, k_vmovhpd_ld7, k_vmovhpd_lda}, {k_vmovhpd_st0, k_vmovhpd_st3, k_vmovhpd_st24, k_vmovhpd_st56}, m_hps_load, 1}};
      static const int ldo[3] = {0, 1, 7}, sto[4] = {0, 3, 24, 56};
      for (int i = 0; i < 4; i++) {
          for (int j = 0; j < 4; j++) { char b[48]; snprintf(b, sizeof b, "%s_xmm_load%s%d", v[i].n, j == 3 ? "_da_o" : "_o", j == 3 ? 0 : ldo[j]); o = newop(nm_(b, ""), v[i].ld[j], v[i].m, 16, EK_I32); o->alias = AL_MEM; o->memoff = j == 3 ? 0 : ldo[j]; o->sz = 8; }
          for (int j = 0; j < 4; j++) { char b[48]; snprintf(b, sizeof b, "%s_store_o%d", v[i].n, sto[j]); o = newop(nm_(b, ""), v[i].st[j], m_store, 16, EK_I32); o->mem_image = 1; o->memoff = sto[j]; o->sz = 8; o->p2 = v[i].hi ? 8 : 0; o->cr = 0; for (int q = 0; q < 4; q++) o->xm[q] = ~0ull; }
      }
      reg3("vmovlhps_xmm", k_vmovlhps_Xdd, k_vmovlhps_Xda, k_vmovlhps_Xdb, k_vmovlhps_Xab, k_vmovlhps_Xal, NULL, 16, EK_I32, m_lhps, NULL, 0, 2, 0);
      reg3("vmovhlps_xmm", k_vmovhlps_Xdd, k_vmovhlps_Xda, k_vmovhlps_Xdb, k_vmovhlps_Xab, k_vmovhlps_Xal, NULL, 16, EK_I32, m_hlps, NULL, 0, 2, 0); }
    // vmovshdup / vmovsldup / vmovddup
    { reg2("vmovshdup_xmm", k_vmovshdup_Xdd, k_vmovshdup_Xda, k_vmovshdup_Xmm, 16, EK_I32, m_dup_r, (void *)(intptr_t)3, 0, 0); reg2("vmovshdup_ymm", k_vmovshdup_Ydd, k_vmovshdup_Yda, k_vmovshdup_Ymm, 32, EK_I32, m_dup_r, (void *)(intptr_t)3, 0, 0);
      reg2("vmovsldup_xmm", k_vmovsldup_Xdd, k_vmovsldup_Xda, k_vmovsldup_Xmm, 16, EK_I32, m_dup_r, (void *)(intptr_t)2, 0, 0); reg2("vmovsldup_ymm", k_vmovsldup_Ydd, k_vmovsldup_Yda, k_vmovsldup_Ymm, 32, EK_I32, m_dup_r, (void *)(intptr_t)2, 0, 0);
      reg2("vmovddup_xmm", k_vmovddup_Xdd, k_vmovddup_Xda, k_vmovddup_Xmm, 16, EK_I64, m_dup_r, (void *)(intptr_t)1, 0, 0); reg2("vmovddup_ymm", k_vmovddup_Ydd, k_vmovddup_Yda, k_vmovddup_Ymm, 32, EK_I64, m_dup_r, (void *)(intptr_t)1, 0, 0);
      for (int i = g_nops - 18; i < g_nops; i++) if (g_ops[i].alias == AL_MEM) g_ops[i].m = m_dup_m;
      o = newop("vmovddup_xmm_mem_o1", k_vmovddup_Xm1, m_dup_m, 16, EK_I64); o->alias = AL_MEM; o->memoff = 1; o->p = (void *)(intptr_t)1; o->nsrc = 1;
      o = newop("vmovddup_xmm_mem_o7", k_vmovddup_Xm7, m_dup_m, 16, EK_I64); o->alias = AL_MEM; o->memoff = 7; o->p = (void *)(intptr_t)1; o->nsrc = 1; }
    // broadcasts
#define REGBC(M, SZ, ISXY) { const char *wn[2] = {"xmm", "ymm"}; \
        KF kr[2][2] = {{k_##M##_Xdd, k_##M##_Xda}, {k_##M##_Ydd, k_##M##_Yda}}; KF km[2][4] = {{k_##M##_Xm0, k_##M##_Xm1, k_##M##_Xm3, k_##M##_Xm15}, {k_##M##_Ym0, k_##M##_Ym1, k_##M##_Ym3, k_##M##_Ym15}}; int mo[4] = {0, 1, 3, 15}; \
        for (int w = 0; w < 2; w++) { if (!(ISXY) && !w) continue; char b[48]; \
            snprintf(b, sizeof b, "%s_%s_from_xmm", #M, wn[w]); o = newop(nm_(b, ""), kr[w][0], m_bcast, w ? 32 : 16, EK_I32); o->sz = SZ; o->nsrc = 1; \
            snprintf(b, sizeof b, "%s_%s_from_xmm_da", #M, wn[w]); o = newop(nm_(b, ""), kr[w][1], m_bcast, w ? 32 : 16, EK_I32); o->sz = SZ; o->nsrc = 1; o->alias = AL_DA; \
            for (int i = 0; i < 4; i++) { snprintf(b, sizeof b, "%s_%s_load_o%d", #M, wn[w], mo[i]); o = newop(nm_(b, ""), km[w][i], m_bcast, w ? 32 : 16, EK_I32); o->sz = SZ; o->nsrc = 1; o->alias = AL_MEM; o->memoff = mo[i]; } } }
    REGBC(vbroadcastss, 4, 1) REGBC(vpbroadcastb, 1, 1) REGBC(vpbroadcastw, 2, 1) REGBC(vpbroadcastd, 4, 1) REGBC(vpbroadcastq, 8, 1)
    o = newop("vbroadcastsd_ymm_from_xmm", k_vbroadcastsd_Ydd, m_bcast, 32, EK_I64); o->sz = 8; o->nsrc = 1;
    o = newop("vbroadcastsd_ymm_from_xmm_da", k_vbroadcastsd_Yda, m_bcast, 32, EK_I64); o->sz = 8; o->nsrc = 1; o->alias = AL_DA;
    { KF k[4] = {k_vbroadcastsd_Ym0, k_vbroadcastsd_Ym1, k_vbroadcastsd_Ym3, k_vbroadcastsd_Ym15}; int mo[4] = {0, 1, 3, 15}; for (int i = 0; i < 4; i++) { char b[48]; snprintf(b, sizeof b, "vbroadcastsd_ymm_load_o%d", mo[i]); o = newop(nm_(b, ""), k[i], m_bcast, 32, EK_I64); o->sz = 8; o->nsrc = 1; o->alias = AL_MEM; o->memoff = mo[i]; } }
    { KF k[3] = {k_vbroadcastf128_Ym0, k_vbroadcastf128_Ym1, k_vbroadcastf128_Ym15}, k2[3] = {k_vbroadcasti128_Ym0, k_vbroadcasti128_Ym1, k_vbroadcasti128_Ym15}; int mo[3] = {0, 1, 15};
      for (int i = 0; i < 3; i++) { char b[48]; snprintf(b, sizeof b, "vbroadcastf128_ymm_load_o%d", mo[i]); o = newop(nm_(b, ""), k[i], m_bcast, 32, EK_I64); o->sz = 16; o->nsrc = 1; o->alias = AL_MEM; o->memoff = mo[i];
          snprintf(b, sizeof b, "vbroadcasti128_ymm_load_o%d", mo[i]); o = newop(nm_(b, ""), k2[i], m_bcast, 32, EK_I64); o->sz = 16; o->nsrc = 1; o->alias = AL_MEM; o->memoff = mo[i]; } }
    // mask extraction
#define REGMSK(M, MOD, EKK) { static const char *gn[4] = {"rax", "rsi", "r9", "r15"}; \
        KF kx[4] = {k_##M##_xrax, k_##M##_xrsi, k_##M##_xr9, k_##M##_xr15}, ky[4] = {k_##M##_yrax, k_##M##_yrsi, k_##M##_yr9, k_##M##_yr15}; \
        for (int i = 0; i < 4; i++) { char b[48]; snprintf(b, sizeof b, "%s_%s_xmm", #M, gn[i]); o = newop(nm_(b, ""), kx[i], MOD, 16, EKK); o->nsrc = 1; xonly(o); \
            snprintf(b, sizeof b, "%s_%s_ymm", #M, gn[i]); o = newop(nm_(b, ""), ky[i], MOD, 32, EKK); o->nsrc = 1; xonly(o); } }
    REGMSK(vmovmskps, m_movmskps, EK_F32) REGMSK(vmovmskpd, m_movmskpd, EK_F64) REGMSK(vpmovmskb, m_pmovmskb, EK_I8)
}
#endif
