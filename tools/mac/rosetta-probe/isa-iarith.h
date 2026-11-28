// Family avx_int_arith: AVX/AVX2 integer SIMD add/sub (wrapping and saturating), min/max, average, compares, multiplies (low/high/rounded/even/pair-add), sad, sign, abs,
// sign/zero extends (vpmovsx/zx), vptest/vtestps/vtestpd flags, shifts (immediate through a 256-stub table, count in xmm, per-element variable, byte shifts), vphminposuw,
// vaddsubps/pd; xmm and ymm, register and memory sources, destination aliasing, page-straddling memory sources.
#ifndef ISA_IARITH_H
#define ISA_IARITH_H
#include "isa-model-iarith.h"
#define IG(M) G3(M)
IG(vpaddb) IG(vpaddw) IG(vpaddd) IG(vpaddq) IG(vpsubb) IG(vpsubw) IG(vpsubd) IG(vpsubq) IG(vpaddsb) IG(vpaddsw) IG(vpaddusb) IG(vpaddusw) IG(vpsubsb) IG(vpsubsw) IG(vpsubusb) IG(vpsubusw) IG(vpminsb) IG(vpminsw) IG(vpminsd) IG(vpminub) IG(vpminuw) IG(vpminud) IG(vpmaxsb) IG(vpmaxsw) IG(vpmaxsd) IG(vpmaxub) IG(vpmaxuw) IG(vpmaxud) IG(vpavgb) IG(vpavgw) IG(vpcmpeqb) IG(vpcmpeqw) IG(vpcmpeqd) IG(vpcmpeqq) IG(vpcmpgtb) IG(vpcmpgtw) IG(vpcmpgtd) IG(vpcmpgtq) IG(vpmullw) IG(vpmulld) IG(vpmulhw) IG(vpmulhuw) IG(vpmulhrsw) IG(vpmuldq) IG(vpmuludq) IG(vpmaddwd) IG(vpmaddubsw) IG(vpsadbw) IG(vpsignb) IG(vpsignw) IG(vpsignd)
IG(vpsllvd) IG(vpsllvq) IG(vpsrlvd) IG(vpsrlvq) IG(vpsravd) IG(vaddsubps) IG(vaddsubpd)
// shifts by an xmm count: the ymm form takes the count from an xmm register: use xmm2 as count in ymm kernels

#define SHX(M) KRN(k_##M##_Xdd, #M " %%xmm2,%%xmm1,%%xmm0", ST0) KRN(k_##M##_Xda, #M " %%xmm2,%%xmm1,%%xmm1", ST1) KRN(k_##M##_Xmm, #M " 64(%0),%%xmm1,%%xmm0", ST0) KRN(k_##M##_Ydd, #M " %%xmm2,%%ymm1,%%ymm0", ST0) KRN(k_##M##_Yda, #M " %%xmm2,%%ymm1,%%ymm1", ST1) KRN(k_##M##_Ymm, #M " 64(%0),%%ymm1,%%ymm0", ST0)
SHX(vpsllw) SHX(vpslld) SHX(vpsllq) SHX(vpsrlw) SHX(vpsrld) SHX(vpsrlq) SHX(vpsraw) SHX(vpsrad)
#define IU(M) G2R(M, "xmm", "xmm", X) G2R(M, "ymm", "ymm", Y)
IU(vpabsb) IU(vpabsw) IU(vpabsd) G2R(vphminposuw, "xmm", "xmm", X)
G2R(vpmovsxbw, "xmm", "xmm", X) G2R(vpmovsxbw, "xmm", "ymm", Y)
G2R(vpmovsxbd, "xmm", "xmm", X) G2R(vpmovsxbd, "xmm", "ymm", Y)
G2R(vpmovsxbq, "xmm", "xmm", X) G2R(vpmovsxbq, "xmm", "ymm", Y)
G2R(vpmovsxwd, "xmm", "xmm", X) G2R(vpmovsxwd, "xmm", "ymm", Y)
G2R(vpmovsxwq, "xmm", "xmm", X) G2R(vpmovsxwq, "xmm", "ymm", Y)
G2R(vpmovsxdq, "xmm", "xmm", X) G2R(vpmovsxdq, "xmm", "ymm", Y)
G2R(vpmovzxbw, "xmm", "xmm", X) G2R(vpmovzxbw, "xmm", "ymm", Y)
G2R(vpmovzxbd, "xmm", "xmm", X) G2R(vpmovzxbd, "xmm", "ymm", Y)
G2R(vpmovzxbq, "xmm", "xmm", X) G2R(vpmovzxbq, "xmm", "ymm", Y)
G2R(vpmovzxwd, "xmm", "xmm", X) G2R(vpmovzxwd, "xmm", "ymm", Y)
G2R(vpmovzxwq, "xmm", "xmm", X) G2R(vpmovzxwq, "xmm", "ymm", Y)
G2R(vpmovzxdq, "xmm", "xmm", X) G2R(vpmovzxdq, "xmm", "ymm", Y)
#define FLKI(NAME, BODY) static void NAME(const IRec *in, Out *out, long n) { \
    A("1:\n" K_PRE "\n " BODY "\n pushfq\n popq 40(%1)\n add $128,%0\n add $64,%1\n dec %2\n jnz 1b" : "+r"(in), "+r"(out), "+r"(n) :: "ymm0", "ymm1", "ymm2", "ymm3", "memory", "cc"); }
FLKI(k_vptest_x, "vptest %%xmm2,%%xmm1") FLKI(k_vptest_y, "vptest %%ymm2,%%ymm1") FLKI(k_vptest_xm, "vptest 64(%0),%%xmm1") FLKI(k_vptest_ym, "vptest 64(%0),%%ymm1") FLKI(k_vptest_a, "vptest %%xmm1,%%xmm1")
FLKI(k_vtestps_x, "vtestps %%xmm2,%%xmm1") FLKI(k_vtestps_y, "vtestps %%ymm2,%%ymm1") FLKI(k_vtestps_xm, "vtestps 64(%0),%%xmm1") FLKI(k_vtestps_ym, "vtestps 64(%0),%%ymm1")
FLKI(k_vtestpd_x, "vtestpd %%xmm2,%%xmm1") FLKI(k_vtestpd_y, "vtestpd %%ymm2,%%ymm1") FLKI(k_vtestpd_xm, "vtestpd 64(%0),%%xmm1") FLKI(k_vtestpd_ym, "vtestpd 64(%0),%%ymm1")
STUB(vpsllw_xdd, "vpsllw $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpsllw_xda, "vpsllw $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpsllw_ydd, "vpsllw $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpsllw_yda, "vpsllw $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpslld_xdd, "vpslld $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpslld_xda, "vpslld $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpslld_ydd, "vpslld $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpslld_yda, "vpslld $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpsllq_xdd, "vpsllq $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpsllq_xda, "vpsllq $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpsllq_ydd, "vpsllq $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpsllq_yda, "vpsllq $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpsrlw_xdd, "vpsrlw $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpsrlw_xda, "vpsrlw $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpsrlw_ydd, "vpsrlw $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpsrlw_yda, "vpsrlw $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpsrld_xdd, "vpsrld $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpsrld_xda, "vpsrld $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpsrld_ydd, "vpsrld $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpsrld_yda, "vpsrld $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpsrlq_xdd, "vpsrlq $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpsrlq_xda, "vpsrlq $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpsrlq_ydd, "vpsrlq $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpsrlq_yda, "vpsrlq $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpsraw_xdd, "vpsraw $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpsraw_xda, "vpsraw $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpsraw_ydd, "vpsraw $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpsraw_yda, "vpsraw $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpsrad_xdd, "vpsrad $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpsrad_xda, "vpsrad $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpsrad_ydd, "vpsrad $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpsrad_yda, "vpsrad $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpslldq_xdd, "vpslldq $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpslldq_xda, "vpslldq $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpslldq_ydd, "vpslldq $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpslldq_yda, "vpslldq $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpsrldq_xdd, "vpsrldq $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpsrldq_xda, "vpsrldq $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpsrldq_ydd, "vpsrldq $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpsrldq_yda, "vpsrldq $\\I,%ymm2,%ymm2", S_ST2)

static Op *ria(const char *name, KF k, const char *kimm, int kind, int es, int opt, int es2, int w, int ek, int alias, int nsrc, int sz, int flt) {
    Op *o = newop(name, k, m_ia, w, ek); o->p = (void *)(intptr_t)(kind | (es << 8) | (opt << 16) | (es2 << 24)); o->kimm = kimm; o->alias = alias; o->nsrc = nsrc; o->sz = sz ? sz : w; o->flt = flt; return o; }
#define RB3(M, KIND, ES, EKK) REG3(M, m_ia, EKK, (void *)(intptr_t)(KIND | (ES << 8)), 0, 2, 0)
#define RSHI(M, ES, OPT) \
    ria(#M "_xmm", NULL, isa_tab_##M##_xdd, II_SHI, ES, OPT, 0, 16, ek_of(ES), AL_DD, 1, 0, 0); ria(#M "_xmm_da", NULL, isa_tab_##M##_xda, II_SHI, ES, OPT, 0, 16, ek_of(ES), AL_DD, 1, 0, 0); \
    ria(#M "_ymm", NULL, isa_tab_##M##_ydd, II_SHI, ES, OPT, 0, 32, ek_of(ES), AL_DD, 1, 0, 0); ria(#M "_ymm_da", NULL, isa_tab_##M##_yda, II_SHI, ES, OPT, 0, 32, ek_of(ES), AL_DD, 1, 0, 0);
#define RSHB(M, OPT) \
    ria(#M "_xmm", NULL, isa_tab_##M##_xdd, II_SHDQ, 1, OPT, 0, 16, EK_I8, AL_DD, 1, 0, 0); ria(#M "_xmm_da", NULL, isa_tab_##M##_xda, II_SHDQ, 1, OPT, 0, 16, EK_I8, AL_DD, 1, 0, 0); \
    ria(#M "_ymm", NULL, isa_tab_##M##_ydd, II_SHDQ, 1, OPT, 0, 32, EK_I8, AL_DD, 1, 0, 0); ria(#M "_ymm_da", NULL, isa_tab_##M##_yda, II_SHDQ, 1, OPT, 0, 32, EK_I8, AL_DD, 1, 0, 0);
#define RSHX(M, ES, OPT) \
    ria(#M "_xmm_count", k_##M##_Xdd, NULL, II_SHX, ES, OPT, 0, 16, ek_of(ES), AL_DD, 2, 0, 0); ria(#M "_xmm_count_da", k_##M##_Xda, NULL, II_SHX, ES, OPT, 0, 16, ek_of(ES), AL_DA, 2, 0, 0); ria(#M "_xmm_count_mem", k_##M##_Xmm, NULL, II_SHX, ES, OPT, 0, 16, ek_of(ES), AL_MEM, 2, 16, 0); \
    ria(#M "_ymm_count", k_##M##_Ydd, NULL, II_SHX, ES, OPT, 0, 32, ek_of(ES), AL_DD, 2, 0, 0); ria(#M "_ymm_count_da", k_##M##_Yda, NULL, II_SHX, ES, OPT, 0, 32, ek_of(ES), AL_DA, 2, 0, 0); ria(#M "_ymm_count_mem", k_##M##_Ymm, NULL, II_SHX, ES, OPT, 0, 32, ek_of(ES), AL_MEM, 2, 16, 0);
#define RU(M, KIND, ES) reg2(#M "_xmm", k_##M##_Xdd, k_##M##_Xda, k_##M##_Xmm, 16, ek_of(ES), m_ia, (void *)(intptr_t)(KIND | (ES << 8)), 0, 0); reg2(#M "_ymm", k_##M##_Ydd, k_##M##_Yda, k_##M##_Ymm, 32, ek_of(ES), m_ia, (void *)(intptr_t)(KIND | (ES << 8)), 0, 0);
#define RPM(M, SIGNED, DE, SE) \
    reg2(#M "_xmm", k_##M##_Xdd, k_##M##_Xda, k_##M##_Xmm, 16, ek_of(SE), m_ia, (void *)(intptr_t)(II_MOVX | (DE << 8) | (SIGNED << 16) | (SE << 24)), 0, 16 / (DE) * (SE)); \
    reg2(#M "_ymm", k_##M##_Ydd, k_##M##_Yda, k_##M##_Ymm, 32, ek_of(SE), m_ia, (void *)(intptr_t)(II_MOVX | (DE << 8) | (SIGNED << 16) | (SE << 24)), 0, 32 / (DE) * (SE));
static uint64_t flm_(void) { const char *e = getenv("PROBE_FLM"); return e ? strtoull(e, NULL, 16) : FL_ALL; }   // PROBE_FLM=<hex mask>: compare only these flag bits (diagnostics)
static void reg_iarith_ops(void) {
    Op *o;
    RB3(vpaddb, II_ADD, 1, EK_I8)
    RB3(vpaddw, II_ADD, 2, EK_I16)
    RB3(vpaddd, II_ADD, 4, EK_I32)
    RB3(vpaddq, II_ADD, 8, EK_I64)
    RB3(vpsubb, II_SUB, 1, EK_I8)
    RB3(vpsubw, II_SUB, 2, EK_I16)
    RB3(vpsubd, II_SUB, 4, EK_I32)
    RB3(vpsubq, II_SUB, 8, EK_I64)
    RB3(vpaddsb, II_ADDS, 1, EK_I8)
    RB3(vpaddsw, II_ADDS, 2, EK_I16)
    RB3(vpaddusb, II_ADDUS, 1, EK_I8)
    RB3(vpaddusw, II_ADDUS, 2, EK_I16)
    RB3(vpsubsb, II_SUBS, 1, EK_I8)
    RB3(vpsubsw, II_SUBS, 2, EK_I16)
    RB3(vpsubusb, II_SUBUS, 1, EK_I8)
    RB3(vpsubusw, II_SUBUS, 2, EK_I16)
    RB3(vpminsb, II_MINS, 1, EK_I8)
    RB3(vpminsw, II_MINS, 2, EK_I16)
    RB3(vpminsd, II_MINS, 4, EK_I32)
    RB3(vpminub, II_MINU, 1, EK_I8)
    RB3(vpminuw, II_MINU, 2, EK_I16)
    RB3(vpminud, II_MINU, 4, EK_I32)
    RB3(vpmaxsb, II_MAXS, 1, EK_I8)
    RB3(vpmaxsw, II_MAXS, 2, EK_I16)
    RB3(vpmaxsd, II_MAXS, 4, EK_I32)
    RB3(vpmaxub, II_MAXU, 1, EK_I8)
    RB3(vpmaxuw, II_MAXU, 2, EK_I16)
    RB3(vpmaxud, II_MAXU, 4, EK_I32)
    RB3(vpavgb, II_AVG, 1, EK_I8)
    RB3(vpavgw, II_AVG, 2, EK_I16)
    RB3(vpcmpeqb, II_CMPEQ, 1, EK_I8)
    RB3(vpcmpeqw, II_CMPEQ, 2, EK_I16)
    RB3(vpcmpeqd, II_CMPEQ, 4, EK_I32)
    RB3(vpcmpeqq, II_CMPEQ, 8, EK_I64)
    RB3(vpcmpgtb, II_CMPGT, 1, EK_I8)
    RB3(vpcmpgtw, II_CMPGT, 2, EK_I16)
    RB3(vpcmpgtd, II_CMPGT, 4, EK_I32)
    RB3(vpcmpgtq, II_CMPGT, 8, EK_I64)
    RB3(vpmullw, II_MULLO, 2, EK_I16)
    RB3(vpmulld, II_MULLO, 4, EK_I32)
    RB3(vpmulhw, II_MULHS, 2, EK_I16)
    RB3(vpmulhuw, II_MULHU, 2, EK_I16)
    RB3(vpmulhrsw, II_MULHRSW, 2, EK_I16)
    RB3(vpmuldq, II_MULDQ, 8, EK_I32)
    RB3(vpmuludq, II_MULUDQ, 8, EK_I32)
    RB3(vpmaddwd, II_MADDWD, 4, EK_I16)
    RB3(vpmaddubsw, II_MADDUBSW, 2, EK_I8)
    RB3(vpsadbw, II_SADBW, 8, EK_I8)
    RB3(vpsignb, II_SIGN, 1, EK_I8)
    RB3(vpsignw, II_SIGN, 2, EK_I16)
    RB3(vpsignd, II_SIGN, 4, EK_I32)
    RB3(vaddsubps, II_ADDSUB, 4, EK_F32) RB3(vaddsubpd, II_ADDSUB, 8, EK_F64)
    for (int i = g_nops - 24; i < g_nops; i++) g_ops[i].flt = 1;
    RSHI(vpsllw, 2, SH_L) RSHI(vpslld, 4, SH_L) RSHI(vpsllq, 8, SH_L) RSHI(vpsrlw, 2, SH_R) RSHI(vpsrld, 4, SH_R) RSHI(vpsrlq, 8, SH_R) RSHI(vpsraw, 2, SH_A) RSHI(vpsrad, 4, SH_A)
    RSHB(vpslldq, SH_L) RSHB(vpsrldq, SH_R)
    RSHX(vpsllw, 2, SH_L) RSHX(vpslld, 4, SH_L) RSHX(vpsllq, 8, SH_L) RSHX(vpsrlw, 2, SH_R) RSHX(vpsrld, 4, SH_R) RSHX(vpsrlq, 8, SH_R) RSHX(vpsraw, 2, SH_A) RSHX(vpsrad, 4, SH_A)
    reg3("vpsllvd_xmm", k_vpsllvd_Xdd, k_vpsllvd_Xda, k_vpsllvd_Xdb, k_vpsllvd_Xab, k_vpsllvd_Xal, k_vpsllvd_Xmm, 16, EK_I32, m_ia, (void *)(intptr_t)(II_SHV | (4 << 8) | (SH_L << 16)), 0, 2, 0);
    reg3("vpsllvd_ymm", k_vpsllvd_Ydd, k_vpsllvd_Yda, k_vpsllvd_Ydb, k_vpsllvd_Yab, k_vpsllvd_Yal, k_vpsllvd_Ymm, 32, EK_I32, m_ia, (void *)(intptr_t)(II_SHV | (4 << 8) | (SH_L << 16)), 0, 2, 0);
    reg3("vpsllvq_xmm", k_vpsllvq_Xdd, k_vpsllvq_Xda, k_vpsllvq_Xdb, k_vpsllvq_Xab, k_vpsllvq_Xal, k_vpsllvq_Xmm, 16, EK_I64, m_ia, (void *)(intptr_t)(II_SHV | (8 << 8) | (SH_L << 16)), 0, 2, 0);
    reg3("vpsllvq_ymm", k_vpsllvq_Ydd, k_vpsllvq_Yda, k_vpsllvq_Ydb, k_vpsllvq_Yab, k_vpsllvq_Yal, k_vpsllvq_Ymm, 32, EK_I64, m_ia, (void *)(intptr_t)(II_SHV | (8 << 8) | (SH_L << 16)), 0, 2, 0);
    reg3("vpsrlvd_xmm", k_vpsrlvd_Xdd, k_vpsrlvd_Xda, k_vpsrlvd_Xdb, k_vpsrlvd_Xab, k_vpsrlvd_Xal, k_vpsrlvd_Xmm, 16, EK_I32, m_ia, (void *)(intptr_t)(II_SHV | (4 << 8) | (SH_R << 16)), 0, 2, 0);
    reg3("vpsrlvd_ymm", k_vpsrlvd_Ydd, k_vpsrlvd_Yda, k_vpsrlvd_Ydb, k_vpsrlvd_Yab, k_vpsrlvd_Yal, k_vpsrlvd_Ymm, 32, EK_I32, m_ia, (void *)(intptr_t)(II_SHV | (4 << 8) | (SH_R << 16)), 0, 2, 0);
    reg3("vpsrlvq_xmm", k_vpsrlvq_Xdd, k_vpsrlvq_Xda, k_vpsrlvq_Xdb, k_vpsrlvq_Xab, k_vpsrlvq_Xal, k_vpsrlvq_Xmm, 16, EK_I64, m_ia, (void *)(intptr_t)(II_SHV | (8 << 8) | (SH_R << 16)), 0, 2, 0);
    reg3("vpsrlvq_ymm", k_vpsrlvq_Ydd, k_vpsrlvq_Yda, k_vpsrlvq_Ydb, k_vpsrlvq_Yab, k_vpsrlvq_Yal, k_vpsrlvq_Ymm, 32, EK_I64, m_ia, (void *)(intptr_t)(II_SHV | (8 << 8) | (SH_R << 16)), 0, 2, 0);
    reg3("vpsravd_xmm", k_vpsravd_Xdd, k_vpsravd_Xda, k_vpsravd_Xdb, k_vpsravd_Xab, k_vpsravd_Xal, k_vpsravd_Xmm, 16, EK_I32, m_ia, (void *)(intptr_t)(II_SHV | (4 << 8) | (SH_A << 16)), 0, 2, 0);
    reg3("vpsravd_ymm", k_vpsravd_Ydd, k_vpsravd_Yda, k_vpsravd_Ydb, k_vpsravd_Yab, k_vpsravd_Yal, k_vpsravd_Ymm, 32, EK_I32, m_ia, (void *)(intptr_t)(II_SHV | (4 << 8) | (SH_A << 16)), 0, 2, 0);
    RU(vpabsb, II_ABS, 1) RU(vpabsw, II_ABS, 2) RU(vpabsd, II_ABS, 4)
    reg2("vphminposuw_xmm", k_vphminposuw_Xdd, k_vphminposuw_Xda, k_vphminposuw_Xmm, 16, EK_I16, m_ia, (void *)(intptr_t)(II_HMINPOS | (2 << 8)), 0, 0);
    RPM(vpmovsxbw, 1, 2, 1) RPM(vpmovsxbd, 1, 4, 1) RPM(vpmovsxbq, 1, 8, 1) RPM(vpmovsxwd, 1, 4, 2) RPM(vpmovsxwq, 1, 8, 2) RPM(vpmovsxdq, 1, 8, 4)
    RPM(vpmovzxbw, 0, 2, 1) RPM(vpmovzxbd, 0, 4, 1) RPM(vpmovzxbq, 0, 8, 1) RPM(vpmovzxwd, 0, 4, 2) RPM(vpmovzxwq, 0, 8, 2) RPM(vpmovzxdq, 0, 8, 4)
#define RFL(NAME, KX, KY, KXM, KYM, KIND, ES, DUP) \
    o = newop(NAME "_xmm", KX, m_ia, 16, EK_I32); o->p = (void *)(intptr_t)(KIND | (ES << 8)); o->cr = 0; o->xm[1] = flm_(); o->nsrc = 2; o->sz = 16; \
    o = newop(NAME "_ymm", KY, m_ia, 32, EK_I32); o->p = (void *)(intptr_t)(KIND | (ES << 8)); o->cr = 0; o->xm[1] = flm_(); o->nsrc = 2; \
    o = newop(NAME "_xmm_mem", KXM, m_ia, 16, EK_I32); o->p = (void *)(intptr_t)(KIND | (ES << 8)); o->cr = 0; o->xm[1] = flm_(); o->nsrc = 2; o->alias = AL_MEM; o->sz = 16; \
    o = newop(NAME "_ymm_mem", KYM, m_ia, 32, EK_I32); o->p = (void *)(intptr_t)(KIND | (ES << 8)); o->cr = 0; o->xm[1] = flm_(); o->nsrc = 2; o->alias = AL_MEM; o->sz = 32;
    RFL("vptest", k_vptest_x, k_vptest_y, k_vptest_xm, k_vptest_ym, II_PTEST, 1, 0) RFL("vtestps", k_vtestps_x, k_vtestps_y, k_vtestps_xm, k_vtestps_ym, II_TESTP, 4, 0) RFL("vtestpd", k_vtestpd_x, k_vtestpd_y, k_vtestpd_xm, k_vtestpd_ym, II_TESTP, 8, 0)
    o = newop("vptest_xmm_same", k_vptest_a, m_ia, 16, EK_I32); o->p = (void *)(intptr_t)(II_PTEST | (1 << 8)); o->cr = 0; o->xm[1] = flm_(); o->nsrc = 2; o->alias = AL_AB;
}
static int plan_ia(const Op *o) {
    int fl = 0; int dd = o->alias == AL_DD; long n = o->kimm ? (dd ? (2L << 20) : (1L << 20)) : (dd ? (8L << 20) : (2L << 20));
    fl += run_stream_op(o, 0x1F80, n, o->exh, NULL);
    if (o->flt) { fl += run_stream_op(o, 0x1F80 | (1 << 13), n / 8, 0, NULL); fl += run_stream_op(o, 0x1F80 | 0x40, n / 8, 0, NULL); fl += run_stream_op(o, 0x1F80 | 0x8000, n / 8, 0, NULL); }
    if (o->alias == AL_MEM && o->al == 0) fl += run_straddle(o);
    return fl;
}
#endif
