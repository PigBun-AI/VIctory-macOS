// Family avx_shuffle: shuffles, permutes, unpacks, packs, blends (immediate and variable), horizontal add/sub, palignr, insert/extract of elements and 128-bit halves,
// vzeroupper. Every immediate-carrying form runs through a table of 256 stubs (one per immediate value) and the engine cycles the immediate over the chunks of records, so
// all 256 values are covered; the data are random with edge values (packs and horizontal sums: saturation boundaries, FP special values).
#ifndef ISA_SHUFFLE_H
#define ISA_SHUFFLE_H
#include "isa-model-shuf.h"

STUB(vshufps_xdd, "vshufps $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vshufps_xda, "vshufps $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vshufps_xdb, "vshufps $\\I,%xmm2,%xmm1,%xmm2", S_ST2)
STUB(vshufps_xmm, "vshufps $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vshufps_ydd, "vshufps $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vshufps_yda, "vshufps $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vshufps_ymm, "vshufps $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vshufpd_xdd, "vshufpd $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vshufpd_xda, "vshufpd $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vshufpd_xdb, "vshufpd $\\I,%xmm2,%xmm1,%xmm2", S_ST2)
STUB(vshufpd_xmm, "vshufpd $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vshufpd_ydd, "vshufpd $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vshufpd_yda, "vshufpd $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vshufpd_ymm, "vshufpd $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vblendps_xdd, "vblendps $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vblendps_xda, "vblendps $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vblendps_xdb, "vblendps $\\I,%xmm2,%xmm1,%xmm2", S_ST2)
STUB(vblendps_xmm, "vblendps $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vblendps_ydd, "vblendps $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vblendps_yda, "vblendps $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vblendps_ymm, "vblendps $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vblendpd_xdd, "vblendpd $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vblendpd_xda, "vblendpd $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vblendpd_xdb, "vblendpd $\\I,%xmm2,%xmm1,%xmm2", S_ST2)
STUB(vblendpd_xmm, "vblendpd $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vblendpd_ydd, "vblendpd $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vblendpd_yda, "vblendpd $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vblendpd_ymm, "vblendpd $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vpblendw_xdd, "vpblendw $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vpblendw_xda, "vpblendw $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vpblendw_xdb, "vpblendw $\\I,%xmm2,%xmm1,%xmm2", S_ST2)
STUB(vpblendw_xmm, "vpblendw $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vpblendw_ydd, "vpblendw $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vpblendw_yda, "vpblendw $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vpblendw_ymm, "vpblendw $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vpblendd_xdd, "vpblendd $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vpblendd_xda, "vpblendd $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vpblendd_xdb, "vpblendd $\\I,%xmm2,%xmm1,%xmm2", S_ST2)
STUB(vpblendd_xmm, "vpblendd $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vpblendd_ydd, "vpblendd $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vpblendd_yda, "vpblendd $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vpblendd_ymm, "vpblendd $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vpalignr_xdd, "vpalignr $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vpalignr_xda, "vpalignr $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vpalignr_xdb, "vpalignr $\\I,%xmm2,%xmm1,%xmm2", S_ST2)
STUB(vpalignr_xmm, "vpalignr $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vpalignr_ydd, "vpalignr $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vpalignr_yda, "vpalignr $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vpalignr_ymm, "vpalignr $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vpshufd_xdd, "vpshufd $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpshufd_xda, "vpshufd $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpshufd_xmm, "vpshufd $\\I,64(%rcx),%xmm0", S_ST0)
STUB(vpshufd_ydd, "vpshufd $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpshufd_yda, "vpshufd $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpshufd_ymm, "vpshufd $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vpshufhw_xdd, "vpshufhw $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpshufhw_xda, "vpshufhw $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpshufhw_xmm, "vpshufhw $\\I,64(%rcx),%xmm0", S_ST0)
STUB(vpshufhw_ydd, "vpshufhw $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpshufhw_yda, "vpshufhw $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpshufhw_ymm, "vpshufhw $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vpshuflw_xdd, "vpshuflw $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpshuflw_xda, "vpshuflw $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpshuflw_xmm, "vpshuflw $\\I,64(%rcx),%xmm0", S_ST0)
STUB(vpshuflw_ydd, "vpshuflw $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpshuflw_yda, "vpshuflw $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpshuflw_ymm, "vpshuflw $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vpermilps_xdd, "vpermilps $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpermilps_xda, "vpermilps $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpermilps_xmm, "vpermilps $\\I,64(%rcx),%xmm0", S_ST0)
STUB(vpermilps_ydd, "vpermilps $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpermilps_yda, "vpermilps $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpermilps_ymm, "vpermilps $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vpermilpd_xdd, "vpermilpd $\\I,%xmm2,%xmm0", S_ST0)
STUB(vpermilpd_xda, "vpermilpd $\\I,%xmm2,%xmm2", S_ST2)
STUB(vpermilpd_xmm, "vpermilpd $\\I,64(%rcx),%xmm0", S_ST0)
STUB(vpermilpd_ydd, "vpermilpd $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpermilpd_yda, "vpermilpd $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpermilpd_ymm, "vpermilpd $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vpermq_ydd, "vpermq $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpermq_yda, "vpermq $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpermq_ymm, "vpermq $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vpermpd_ydd, "vpermpd $\\I,%ymm2,%ymm0", S_ST0)
STUB(vpermpd_yda, "vpermpd $\\I,%ymm2,%ymm2", S_ST2)
STUB(vpermpd_ymm, "vpermpd $\\I,64(%rcx),%ymm0", S_ST0)
STUB(vperm2f128_ydd, "vperm2f128 $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vperm2f128_yda, "vperm2f128 $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vperm2f128_ymm, "vperm2f128 $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vperm2i128_ydd, "vperm2i128 $\\I,%ymm2,%ymm1,%ymm0", S_ST0)
STUB(vperm2i128_yda, "vperm2i128 $\\I,%ymm2,%ymm1,%ymm1", S_ST1)
STUB(vperm2i128_ymm, "vperm2i128 $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vinsertf128_ydd, "vinsertf128 $\\I,%xmm2,%ymm1,%ymm0", S_ST0)
STUB(vinsertf128_yda, "vinsertf128 $\\I,%xmm2,%ymm1,%ymm1", S_ST1)
STUB(vinsertf128_ymm, "vinsertf128 $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vinserti128_ydd, "vinserti128 $\\I,%xmm2,%ymm1,%ymm0", S_ST0)
STUB(vinserti128_yda, "vinserti128 $\\I,%xmm2,%ymm1,%ymm1", S_ST1)
STUB(vinserti128_ymm, "vinserti128 $\\I,64(%rcx),%ymm1,%ymm0", S_ST0)
STUB(vextractf128_xdd, "vextractf128 $\\I,%ymm2,%xmm0", S_ST0)
STUB(vextractf128_m, "vmovups %ymm0,(%rdx)\n vextractf128 $\\I,%ymm2,3(%rdx)", "")
STUB(vextracti128_xdd, "vextracti128 $\\I,%ymm2,%xmm0", S_ST0)
STUB(vextracti128_m, "vmovups %ymm0,(%rdx)\n vextracti128 $\\I,%ymm2,3(%rdx)", "")
STUB(vinsertps_xdd, "vinsertps $\\I,%xmm2,%xmm1,%xmm0", S_ST0)
STUB(vinsertps_xda, "vinsertps $\\I,%xmm2,%xmm1,%xmm1", S_ST1)
STUB(vinsertps_xmm, "vinsertps $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vpinsrb_xdd, "mov 64(%rcx),%rax\n vpinsrb $\\I,%eax,%xmm1,%xmm0", S_ST0)
STUB(vpinsrb_xmm, "vpinsrb $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vpinsrw_xdd, "mov 64(%rcx),%rax\n vpinsrw $\\I,%eax,%xmm1,%xmm0", S_ST0)
STUB(vpinsrw_xmm, "vpinsrw $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vpinsrd_xdd, "mov 64(%rcx),%rax\n vpinsrd $\\I,%eax,%xmm1,%xmm0", S_ST0)
STUB(vpinsrd_xmm, "vpinsrd $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vpinsrq_xdd, "mov 64(%rcx),%rax\n vpinsrq $\\I,%rax,%xmm1,%xmm0", S_ST0)
STUB(vpinsrq_xmm, "vpinsrq $\\I,64(%rcx),%xmm1,%xmm0", S_ST0)
STUB(vpextrb_r, "mov $-1,%rax\n vpextrb $\\I,%xmm2,%eax", "mov %rax,32(%rdx)")
STUB(vpextrb_m, "vmovups %ymm0,(%rdx)\n vpextrb $\\I,%xmm2,3(%rdx)", "")
STUB(vpextrw_r, "mov $-1,%rax\n vpextrw $\\I,%xmm2,%eax", "mov %rax,32(%rdx)")
STUB(vpextrw_m, "vmovups %ymm0,(%rdx)\n vpextrw $\\I,%xmm2,3(%rdx)", "")
STUB(vpextrd_r, "mov $-1,%rax\n vpextrd $\\I,%xmm2,%eax", "mov %rax,32(%rdx)")
STUB(vpextrd_m, "vmovups %ymm0,(%rdx)\n vpextrd $\\I,%xmm2,3(%rdx)", "")
STUB(vpextrq_r, "mov $-1,%rax\n vpextrq $\\I,%xmm2,%rax", "mov %rax,32(%rdx)")
STUB(vpextrq_m, "vmovups %ymm0,(%rdx)\n vpextrq $\\I,%xmm2,3(%rdx)", "")
STUB(vextractps_r, "mov $-1,%rax\n vextractps $\\I,%xmm2,%eax", "mov %rax,32(%rdx)")
STUB(vextractps_m, "vmovups %ymm0,(%rdx)\n vextractps $\\I,%xmm2,3(%rdx)", "")
// register / alias forms without immediate: M rm, vvvv, dst
#define SG(M) G3(M)
SG(vunpcklps) SG(vunpckhps) SG(vunpcklpd) SG(vunpckhpd) SG(vpunpcklbw) SG(vpunpcklwd) SG(vpunpckldq) SG(vpunpcklqdq) SG(vpunpckhbw) SG(vpunpckhwd) SG(vpunpckhdq) SG(vpunpckhqdq)
SG(vpackssdw) SG(vpacksswb) SG(vpackuswb) SG(vpackusdw) SG(vpshufb) SG(vhaddps) SG(vhaddpd) SG(vhsubps) SG(vhsubpd) SG(vphaddw) SG(vphaddd) SG(vphsubw) SG(vphsubd) SG(vphaddsw) SG(vphsubsw)
SG(vpermilps) SG(vpermilpd)

#define KRN4(NAME, BODY) KRN(NAME, BODY, ST0)
#define BV(M) KRN(k_##M##_xdd, #M " %%xmm3,%%xmm2,%%xmm1,%%xmm0", ST0) KRN(k_##M##_xda, #M " %%xmm3,%%xmm2,%%xmm1,%%xmm1", ST1) KRN(k_##M##_xdc, #M " %%xmm3,%%xmm2,%%xmm1,%%xmm3", "vmovups %%ymm3,(%1)") KRN(k_##M##_xmm, #M " %%xmm3,64(%0),%%xmm1,%%xmm0", ST0) \
    KRN(k_##M##_ydd, #M " %%ymm3,%%ymm2,%%ymm1,%%ymm0", ST0) KRN(k_##M##_yda, #M " %%ymm3,%%ymm2,%%ymm1,%%ymm1", ST1) KRN(k_##M##_ymm, #M " %%ymm3,64(%0),%%ymm1,%%ymm0", ST0)
BV(vblendvps) BV(vblendvpd) BV(vpblendvb)
KRN(k_vpermd_ydd, "vpermd %%ymm2,%%ymm1,%%ymm0", ST0) KRN(k_vpermd_yda, "vpermd %%ymm2,%%ymm1,%%ymm1", ST1) KRN(k_vpermd_ymm, "vpermd 64(%0),%%ymm1,%%ymm0", ST0)
KRN(k_vpermps_ydd, "vpermps %%ymm2,%%ymm1,%%ymm0", ST0) KRN(k_vpermps_yda, "vpermps %%ymm2,%%ymm1,%%ymm1", ST1) KRN(k_vpermps_ymm, "vpermps 64(%0),%%ymm1,%%ymm0", ST0)
// vzeroupper: ymm0 <- d, ymm8 <- a; after vzeroupper ymm0 -> r, ymm8 -> x
static void k_vzeroupper(const IRec *in, Out *out, long n) {
    A("1:\n vmovups (%0),%%ymm0\n vmovups 32(%0),%%ymm8\n vzeroupper\n vmovups %%ymm0,(%1)\n vmovups %%ymm8,32(%1)" K_POST : "+r"(in), "+r"(out), "+r"(n) :: "ymm0", "ymm8", "memory", "cc"); }

static Op *rsh(const char *name, KF k, const char *kimm, int kind, int es, int opt, int w, int ek, int alias, int nsrc, int sz, int flt) {
    Op *o = newop(name, k, m_shuf, w, ek); o->p = (void *)(intptr_t)(kind | (es << 8) | (opt << 16)); o->kimm = kimm; o->alias = alias; o->nsrc = nsrc; o->sz = sz ? sz : w; o->flt = flt; return o;
}
static int ek_of(int es) { return es == 1 ? EK_I8 : es == 2 ? EK_I16 : es == 4 ? EK_I32 : EK_I64; }
#define IT(M, S) isa_tab_##M##_##S
#define RI3(M, KIND, ES, OPT) \
    rsh(#M "_xmm", NULL, IT(M, xdd), KIND, ES, OPT, 16, ek_of(ES), AL_DD, 2, 0, 0); rsh(#M "_xmm_da", NULL, IT(M, xda), KIND, ES, OPT, 16, ek_of(ES), AL_DA, 2, 0, 0); \
    rsh(#M "_xmm_db", NULL, IT(M, xdb), KIND, ES, OPT, 16, ek_of(ES), AL_DB, 2, 0, 0); rsh(#M "_xmm_mem", NULL, IT(M, xmm), KIND, ES, OPT, 16, ek_of(ES), AL_MEM, 2, 16, 0); \
    rsh(#M "_ymm", NULL, IT(M, ydd), KIND, ES, OPT, 32, ek_of(ES), AL_DD, 2, 0, 0); rsh(#M "_ymm_da", NULL, IT(M, yda), KIND, ES, OPT, 32, ek_of(ES), AL_DA, 2, 0, 0); rsh(#M "_ymm_mem", NULL, IT(M, ymm), KIND, ES, OPT, 32, ek_of(ES), AL_MEM, 2, 32, 0);
#define RI2(M, KIND, ES) \
    rsh(#M "_xmm", NULL, IT(M, xdd), KIND, ES, 0, 16, ek_of(ES), AL_DD, 1, 0, 0); rsh(#M "_xmm_da", NULL, IT(M, xda), KIND, ES, 0, 16, ek_of(ES), AL_DA, 1, 0, 0); rsh(#M "_xmm_mem", NULL, IT(M, xmm), KIND, ES, 0, 16, ek_of(ES), AL_MEM, 1, 16, 0); \
    rsh(#M "_ymm", NULL, IT(M, ydd), KIND, ES, 0, 32, ek_of(ES), AL_DD, 1, 0, 0); rsh(#M "_ymm_da", NULL, IT(M, yda), KIND, ES, 0, 32, ek_of(ES), AL_DA, 1, 0, 0); rsh(#M "_ymm_mem", NULL, IT(M, ymm), KIND, ES, 0, 32, ek_of(ES), AL_MEM, 1, 32, 0);
#define RIY(M, KIND, ES, NS) \
    rsh(#M "_ymm", NULL, IT(M, ydd), KIND, ES, 0, 32, ek_of(ES), AL_DD, NS, 0, 0); rsh(#M "_ymm_da", NULL, IT(M, yda), KIND, ES, 0, 32, ek_of(ES), AL_DA, NS, 0, 0); rsh(#M "_ymm_mem", NULL, IT(M, ymm), KIND, ES, 0, 32, ek_of(ES), AL_MEM, NS, 32, 0);
#define RV(M, KIND, ES, OPT, EKK, FLT) REG3(M, m_shuf, EKK, (void *)(intptr_t)(KIND | (ES << 8) | (OPT << 16)), FLT, 2, 0)
#define RBV(M, ES) \
    { void *pp = (void *)(intptr_t)(S_BLENDV | (ES << 8)); Op *o; \
      o = newop(#M "_xmm", k_##M##_xdd, m_shuf, 16, ek_of(ES)); o->p = pp; o->nsrc = 3; o = newop(#M "_xmm_da", k_##M##_xda, m_shuf, 16, ek_of(ES)); o->p = pp; o->nsrc = 3; o->alias = AL_DA; \
      o = newop(#M "_xmm_dc", k_##M##_xdc, m_shuf, 16, ek_of(ES)); o->p = pp; o->nsrc = 3; o->alias = AL_DB; o = newop(#M "_xmm_mem", k_##M##_xmm, m_shuf, 16, ek_of(ES)); o->p = pp; o->nsrc = 3; o->alias = AL_MEM; o->sz = 16; \
      o = newop(#M "_ymm", k_##M##_ydd, m_shuf, 32, ek_of(ES)); o->p = pp; o->nsrc = 3; o = newop(#M "_ymm_da", k_##M##_yda, m_shuf, 32, ek_of(ES)); o->p = pp; o->nsrc = 3; o->alias = AL_DA; \
      o = newop(#M "_ymm_mem", k_##M##_ymm, m_shuf, 32, ek_of(ES)); o->p = pp; o->nsrc = 3; o->alias = AL_MEM; o->sz = 32; }
static void m_vzu(const Ctx *c, const IRec *r, Out *o) { (void)c; memset(o->r, 0, 32); memcpy(o->r, r->d, 16); memcpy(o->x, r->a, 16); }
static void reg_shuffle_ops(void) {
    Op *o; (void)o;
    RI3(vshufps, S_SHUFPS, 4, 0) RI3(vshufpd, S_SHUFPD, 8, 0) RI3(vblendps, S_BLENDI, 4, 0) RI3(vblendpd, S_BLENDI, 8, 0) RI3(vpblendw, S_BLENDI, 2, 0) RI3(vpblendd, S_BLENDI, 4, 0) RI3(vpalignr, S_PALIGNR, 1, 0)
    RI2(vpshufd, S_PSHUFD, 4) RI2(vpshufhw, S_PSHUFHW, 2) RI2(vpshuflw, S_PSHUFLW, 2) RI2(vpermilps, S_PERMILPS_I, 4) RI2(vpermilpd, S_PERMILPD_I, 8)
    RIY(vpermq, S_PERMQ, 8, 1) RIY(vpermpd, S_PERMQ, 8, 1) RIY(vperm2f128, S_PERM2X128, 16, 2) RIY(vperm2i128, S_PERM2X128, 16, 2) RIY(vinsertf128, S_INS128, 16, 2) RIY(vinserti128, S_INS128, 16, 2)
    // vinsert*128 and vperm2*128 data are bytes: the "element size" 16 only picks EK_I64 below
    for (int i = 0; i < g_nops; i++) if (g_ops[i].ek == EK_I64 + 0 && 0) {}
    rsh("vextractf128_xmm_ymm", NULL, IT(vextractf128, xdd), S_EXT128, 16, 0, 32, EK_I64, AL_DD, 1, 0, 0); rsh("vextracti128_xmm_ymm", NULL, IT(vextracti128, xdd), S_EXT128, 16, 0, 32, EK_I64, AL_DD, 1, 0, 0);
    o = rsh("vextractf128_m128_ymm", NULL, IT(vextractf128, m), S_EXT128, 16, 0, 32, EK_I64, AL_DD, 1, 16, 0); o->mem_r = 1; o->memoff = 3; o = rsh("vextracti128_m128_ymm", NULL, IT(vextracti128, m), S_EXT128, 16, 0, 32, EK_I64, AL_DD, 1, 16, 0); o->mem_r = 1; o->memoff = 3;
    rsh("vinsertps_xmm", NULL, IT(vinsertps, xdd), S_INSERTPS, 4, 0, 16, EK_I32, AL_DD, 2, 0, 0); rsh("vinsertps_xmm_da", NULL, IT(vinsertps, xda), S_INSERTPS, 4, 0, 16, EK_I32, AL_DA, 2, 0, 0); rsh("vinsertps_xmm_m32", NULL, IT(vinsertps, xmm), S_INSERTPS, 4, 0, 16, EK_I32, AL_MEM, 2, 4, 0);
#define RPI(M, ES) rsh(#M "_x_r", NULL, IT(M, xdd), S_PINSR, ES, 0, 16, ek_of(ES), AL_DD, 2, 0, 0); rsh(#M "_x_m", NULL, IT(M, xmm), S_PINSR, ES, 0, 16, ek_of(ES), AL_MEM, 2, ES, 0);
    RPI(vpinsrb, 1) RPI(vpinsrw, 2) RPI(vpinsrd, 4) RPI(vpinsrq, 8)
#define RPX(M, ES) o = rsh(#M "_r_x", NULL, IT(M, r), S_PEXTR, ES, 0, 16, ek_of(ES), AL_DD, 1, 0, 0); o->cr = 0; o->xm[0] = ~0ull; \
                   o = rsh(#M "_m_x", NULL, IT(M, m), S_PEXTR, ES, 0, 16, ek_of(ES), AL_DD, 1, ES, 0); o->mem_r = 1; o->memoff = 3;
    RPX(vpextrb, 1) RPX(vpextrw, 2) RPX(vpextrd, 4) RPX(vpextrq, 8) RPX(vextractps, 4)
    RV(vunpcklps, S_UNPCK, 4, 0, EK_I32, 0) RV(vunpckhps, S_UNPCK, 4, 1, EK_I32, 0) RV(vunpcklpd, S_UNPCK, 8, 0, EK_I64, 0) RV(vunpckhpd, S_UNPCK, 8, 1, EK_I64, 0)
    RV(vpunpcklbw, S_UNPCK, 1, 0, EK_I8, 0) RV(vpunpckhbw, S_UNPCK, 1, 1, EK_I8, 0) RV(vpunpcklwd, S_UNPCK, 2, 0, EK_I16, 0) RV(vpunpckhwd, S_UNPCK, 2, 1, EK_I16, 0)
    RV(vpunpckldq, S_UNPCK, 4, 0, EK_I32, 0) RV(vpunpckhdq, S_UNPCK, 4, 1, EK_I32, 0) RV(vpunpcklqdq, S_UNPCK, 8, 0, EK_I64, 0) RV(vpunpckhqdq, S_UNPCK, 8, 1, EK_I64, 0)
    RV(vpackssdw, S_PACK, 2, 0, EK_I32, 0) RV(vpacksswb, S_PACK, 1, 1, EK_I16, 0) RV(vpackuswb, S_PACK, 1, 2, EK_I16, 0) RV(vpackusdw, S_PACK, 2, 3, EK_I32, 0)
    RV(vpshufb, S_PSHUFB, 1, 0, EK_I8, 0)
    RV(vhaddps, S_HADD, 4, 0, EK_F32, 1) RV(vhsubps, S_HADD, 4, 1, EK_F32, 1) RV(vhaddpd, S_HADD, 8, 0, EK_F64, 1) RV(vhsubpd, S_HADD, 8, 1, EK_F64, 1)
    RV(vphaddw, S_PHADD, 2, 0, EK_I16, 0) RV(vphsubw, S_PHADD, 2, 1, EK_I16, 0) RV(vphaddsw, S_PHADD, 2, 2, EK_I16, 0) RV(vphsubsw, S_PHADD, 2, 3, EK_I16, 0) RV(vphaddd, S_PHADD, 4, 0, EK_I32, 0) RV(vphsubd, S_PHADD, 4, 1, EK_I32, 0)
    reg3("vpermilps_var_xmm", k_vpermilps_Xdd, k_vpermilps_Xda, k_vpermilps_Xdb, k_vpermilps_Xab, k_vpermilps_Xal, k_vpermilps_Xmm, 16, EK_I32, m_shuf, (void *)(intptr_t)(S_PERMILPS_V | (4 << 8)), 0, 2, 0);
    reg3("vpermilps_var_ymm", k_vpermilps_Ydd, k_vpermilps_Yda, k_vpermilps_Ydb, k_vpermilps_Yab, k_vpermilps_Yal, k_vpermilps_Ymm, 32, EK_I32, m_shuf, (void *)(intptr_t)(S_PERMILPS_V | (4 << 8)), 0, 2, 0);
    reg3("vpermilpd_var_xmm", k_vpermilpd_Xdd, k_vpermilpd_Xda, k_vpermilpd_Xdb, k_vpermilpd_Xab, k_vpermilpd_Xal, k_vpermilpd_Xmm, 16, EK_I64, m_shuf, (void *)(intptr_t)(S_PERMILPD_V | (8 << 8)), 0, 2, 0);
    reg3("vpermilpd_var_ymm", k_vpermilpd_Ydd, k_vpermilpd_Yda, k_vpermilpd_Ydb, k_vpermilpd_Yab, k_vpermilpd_Yal, k_vpermilpd_Ymm, 32, EK_I64, m_shuf, (void *)(intptr_t)(S_PERMILPD_V | (8 << 8)), 0, 2, 0);
    RBV(vblendvps, 4) RBV(vblendvpd, 8) RBV(vpblendvb, 1)
    o = newop("vpermd_ymm", k_vpermd_ydd, m_shuf, 32, EK_I32); o->p = (void *)(intptr_t)(S_PERMD | (4 << 8)); o = newop("vpermd_ymm_da", k_vpermd_yda, m_shuf, 32, EK_I32); o->p = (void *)(intptr_t)(S_PERMD | (4 << 8)); o->alias = AL_DA;
    o = newop("vpermd_ymm_mem", k_vpermd_ymm, m_shuf, 32, EK_I32); o->p = (void *)(intptr_t)(S_PERMD | (4 << 8)); o->alias = AL_MEM; o->sz = 32;
    o = newop("vpermps_ymm", k_vpermps_ydd, m_shuf, 32, EK_I32); o->p = (void *)(intptr_t)(S_PERMD | (4 << 8)); o = newop("vpermps_ymm_da", k_vpermps_yda, m_shuf, 32, EK_I32); o->p = (void *)(intptr_t)(S_PERMD | (4 << 8)); o->alias = AL_DA;
    o = newop("vpermps_ymm_mem", k_vpermps_ymm, m_shuf, 32, EK_I32); o->p = (void *)(intptr_t)(S_PERMD | (4 << 8)); o->alias = AL_MEM; o->sz = 32;
    o = newop("vzeroupper", k_vzeroupper, m_vzu, 32, EK_I8); o->xm[0] = o->xm[1] = o->xm[2] = o->xm[3] = ~0ull;
}
static int plan_shuf(const Op *o) {
    if (o->flt) return plan_fp(o);
    int fl = 0; int dd = o->alias == AL_DD; long n = o->kimm ? (dd ? (3L << 20) : (1L << 20)) : (dd ? (8L << 20) : (2L << 20));
    fl += run_stream_op(o, 0x1F80, n, 0, NULL);
    if ((o->alias == AL_MEM || o->mem_r) && o->al == 0) fl += run_straddle(o);
    return fl;
}
#endif
