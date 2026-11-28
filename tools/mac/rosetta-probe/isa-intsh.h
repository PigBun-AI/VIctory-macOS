// Family x86_int_shift_mul_div: shl/shr/sar/rol/ror (count in cl and immediate, counts 0 / >= width, register and memory), shld/shrd, mul/imul (1, 2, 3 operands) with
// CF/OF, div/idiv (operands generated so that the quotient fits: no #DE), bsf/bsr/tzcnt/lzcnt/popcnt, bt/bts/btr/btc, bswap, movbe, cbw/cwde/cdqe/cwd/cdq/cqo, and the BMI
// instructions (andn bextr bzhi blsr blsi pext pdep mulx shlx shrx sarx rorx). Flags the SDM calls undefined are ignored per record (umask).
#ifndef ISA_INTSH_H
#define ISA_INTSH_H
#include "isa-intmove.h"
#include "isa-model-intsh.h"
enum { G_SH, G_MD, G_BS, G_BT, G_BMI, G_CVT };
enum { BM_ANDN, BM_BEXTR, BM_BZHI, BM_BLSR, BM_BLSI, BM_PEXT, BM_PDEP, BM_MULX, BM_SHLX, BM_SHRX, BM_SARX, BM_RORX, BM_BSWAP, BM_MOVBE_LD, BM_MOVBE_ST };
// op->p: kind | wb << 8 | dst << 16 | src << 20 | grp << 24.  dst: 0 rax, 1 rbx, 2 memory (out.r + memoff).  src: 0 rax, 1 rbx, 2 memory (record b field + memoff), 3 immediate (op->imm), 5 cl (rcx), 6 rcx as a plain source
static void eval_sh(const Ctx *c, const IRec *r, Out *o, uint64_t um[4]) {
    int pv = (int)(intptr_t)c->op->p, kind = pv & 0xFF, wb = (pv >> 8) & 15, dst = (pv >> 16) & 15, src = (pv >> 20) & 15, grp = (pv >> 24) & 15, w = wb * 8; uint64_t m = mskb(w);
    uint64_t rax = ld_(r->a, 8), rbx = ld_(r->b, 8), rdx = ld_(r->d, 8), rcx = ld_(r->c, 8); uint32_t fin = (uint32_t)ld_(r->c + 8, 8) & FL_ALL, f = fin, und = 0;
    uint64_t dold = dst == 0 ? rax : dst == 1 ? rbx : ld_(r->d + c->op->memoff, wb), newv = dold; int wr = 1; um[0] = um[1] = um[2] = um[3] = 0;
    
    switch (grp) {
    case G_SH: { uint64_t cnt = src == 3 ? (uint64_t)c->op->imm & 0xFF : rcx & 0xFF; AluR a = shift_op(kind, wb, dold, rbx, cnt, fin, &und); newv = a.r; f = a.f; break; }
    case G_MD: {
        uint64_t ra = rax, rd = rdx;
        if (kind == MD_IMUL2) { uint64_t sx = src == 2 ? ld_(msrc(r, c->op), wb) : src == 0 ? rax : rbx; muldiv(kind, wb, &ra, &rd, sx, &f, &und); rax = ra; wr = 0; }
        else if (kind == MD_IMUL3) { uint64_t t = src == 2 ? ld_(msrc(r, c->op), wb) : rbx; ra = t; muldiv(MD_IMUL2, wb, &ra, &rd, (uint64_t)(int64_t)c->op->imm, &f, &und); rax = wb == 8 ? ra : wb == 4 ? ra : (rax & ~m) | (ra & m); wr = 0; }
        else { uint64_t sx = src == 2 ? ld_(msrc(r, c->op), wb) : rbx; muldiv(kind, wb, &ra, &rd, sx, &f, &und); rax = ra; rdx = rd; wr = 0; }
        if (kind == MD_DIV || kind == MD_IDIV) f = fin; else f = (f & (FL_CF | FL_OF)) | (fin & ~(uint32_t)(FL_CF | FL_OF));
        break; }
    case G_BS: { int zs; uint64_t s = src == 2 ? ld_(msrc(r, c->op), wb) : src == 0 ? rax : rbx; AluR a = bitscan(kind, wb, s, dold, &und, &zs); newv = a.r; f = a.f; if (zs) um[0] = ~0ull; break; }
    case G_BT: { uint64_t idx = src == 3 ? (uint64_t)c->op->imm & 0xFF : rbx; AluR a = bittest(kind & 3, wb, dold, idx, fin, &und); newv = a.r; f = a.f; if (kind == 0) wr = dst == 2 ? 0 : 0; break; }
    case G_BMI: {
        uint64_t s2 = src == 2 ? ld_(msrc(r, c->op), wb) : rcx; s2 &= m; uint64_t rb = rbx & m;
        switch (kind) {
        case BM_ANDN: newv = ~rb & s2 & m; f = (newv == 0 ? FL_ZF : 0) | ((newv >> (w - 1)) & 1 ? FL_SF : 0); und = FL_AF | FL_PF; break;
        case BM_BEXTR: { uint64_t ctl = rcx, st = ctl & 0xFF, ln = (ctl >> 8) & 0xFF, s = src == 2 ? ld_(msrc(r, c->op), wb) & m : rb; uint64_t v = st >= (uint64_t)w ? 0 : s >> st; if (ln < (uint64_t)w) v &= (1ull << ln) - 1; newv = v & m; f = newv == 0 ? FL_ZF : 0; und = FL_SF | FL_AF | FL_PF; break; }
        case BM_BZHI: { uint64_t idx = rcx & 0xFF, s = src == 2 ? ld_(msrc(r, c->op), wb) & m : rb; newv = idx >= (uint64_t)w ? s : s & ((1ull << idx) - 1); f = (idx >= (uint64_t)w ? FL_CF : 0) | (newv == 0 ? FL_ZF : 0) | ((newv >> (w - 1)) & 1 ? FL_SF : 0); und = FL_AF | FL_PF; break; }
        case BM_BLSR: { uint64_t s = src == 2 ? ld_(msrc(r, c->op), wb) & m : rb; newv = s & (s - 1) & m; f = (s == 0 ? FL_CF : 0) | (newv == 0 ? FL_ZF : 0) | ((newv >> (w - 1)) & 1 ? FL_SF : 0); und = FL_AF | FL_PF; break; }
        case BM_BLSI: { uint64_t s = src == 2 ? ld_(msrc(r, c->op), wb) & m : rb; newv = s & (0 - s) & m; f = (s != 0 ? FL_CF : 0) | (newv == 0 ? FL_ZF : 0) | ((newv >> (w - 1)) & 1 ? FL_SF : 0); und = FL_AF | FL_PF; break; }
        case BM_PEXT: newv = pext_n(rb, rcx & m, w); break;
        case BM_PDEP: newv = pdep_n(rb, rcx & m, w); break;
        case BM_MULX: { uint64_t s = src == 2 ? ld_(msrc(r, c->op), wb) & m : rcx & m; u128_t p = (u128_t)(rdx & m) * s; newv = (uint64_t)(p >> w) & m; rbx = (uint64_t)p & m; break; }
        case BM_SHLX: case BM_SHRX: case BM_SARX: { uint64_t s = src == 2 ? ld_(msrc(r, c->op), wb) & m : rb; uint32_t n = (uint32_t)(rcx & (wb == 8 ? 63 : 31)); newv = kind == BM_SHLX ? (s << n) & m : kind == BM_SHRX ? s >> n : (uint64_t)(sx_(s, w) >> n) & m; break; }
        case BM_RORX: { uint64_t s = src == 2 ? ld_(msrc(r, c->op), wb) & m : rb; uint32_t n = (uint32_t)((uint64_t)c->op->imm & (w - 1)); newv = n ? ((s >> n) | (s << (w - n))) & m : s; break; }
        case BM_BSWAP: newv = bswap_n(dold, wb); break;
        case BM_MOVBE_LD: newv = bswap_n(ld_(msrc(r, c->op), wb), wb); break;
        case BM_MOVBE_ST: newv = bswap_n(rax, wb); break;
        }
        if (kind == BM_MOVBE_ST) { st_(o->r + c->op->memoff, wb, newv); wr = 0; }
        break; }
    case G_CVT: { int64_t s = sx_(kind == 0 ? rax & 0xFF : kind == 1 ? rax & 0xFFFF : kind == 2 ? rax & 0xFFFFFFFFull : rax, kind == 0 ? 8 : kind == 1 ? 16 : kind == 2 ? 32 : 64);       // kind: 0 cbw, 1 cwde, 2 cdqe, 3 cwd, 4 cdq, 5 cqo
        if (kind <= 2) { int bw = kind == 0 ? 2 : kind == 1 ? 4 : 8; rax = put_reg(rax, bw, (uint64_t)s); }
        else { int sw = kind == 3 ? 2 : kind == 4 ? 4 : 8; int64_t sv2 = sx_(sw == 2 ? rax & 0xFFFF : sw == 4 ? rax & 0xFFFFFFFFull : rax, sw * 8); rdx = put_reg(rdx, sw, sv2 < 0 ? ~0ull : 0); }
        wr = 0; break; }
    }
    if (wr) {
        int nowrite = (grp == G_BT && kind == 0);
        if (!nowrite) { if (dst == 2) st_(o->r + c->op->memoff, wb, newv); else { uint64_t nv = put_reg(dst == 0 ? rax : rbx, wb, newv); if (dst == 0) rax = nv; else rbx = nv; } }
    }
    o->x[0] = rax; o->x[1] = f; o->x[2] = rdx; o->x[3] = rbx; um[1] = und;
}
static void m_intsh(const Ctx *c, const IRec *r, Out *o) { uint64_t um[4]; eval_sh(c, r, o, um); }
static void um_intsh(const IRec *r, const Op *op, uint64_t um[4]) { Ctx cx = {op, op->w, op->imm, 0}; Out o; memset(&o, 0, sizeof o); eval_sh(&cx, r, &o, um); }
// the kernels
#define SHK(M, W, RA, RB, SFX) \
    INK(k_##M##_##W##_cl, #M " %%cl,%%" RA) INK(k_##M##_##W##_clb, #M " %%cl,%%" RB) INK(k_##M##_##W##_mcl, #M SFX " %%cl,3(%1)") \
    INK(k_##M##_##W##_i1, #M " $1,%%" RA) INK(k_##M##_##W##_i2, #M " $2,%%" RA) INK(k_##M##_##W##_i7, #M " $7,%%" RA) INK(k_##M##_##W##_i8, #M " $8,%%" RA) \
    INK(k_##M##_##W##_i16, #M " $16,%%" RB) INK(k_##M##_##W##_i31, #M " $31,%%" RA) INK(k_##M##_##W##_i33, #M " $33,%%" RB) INK(k_##M##_##W##_i63, #M " $63,%%" RA) \
    INK(k_##M##_##W##_mi5, #M SFX " $5,3(%1)") INK(k_##M##_##W##_mi1, #M SFX " $1,(%1)")
#define SHKALL(M) SHK(M, 8, "al", "bl", "b") SHK(M, 16, "ax", "bx", "w") SHK(M, 32, "eax", "ebx", "l") SHK(M, 64, "rax", "rbx", "q")
SHKALL(shl) SHKALL(shr) SHKALL(sar) SHKALL(rol) SHKALL(ror)
#define DSK(M, W, RA, RB, SFX) INK(k_##M##_##W##_cl, #M " %%cl,%%" RB ",%%" RA) INK(k_##M##_##W##_mcl, #M " %%cl,%%" RB ",3(%1)") INK(k_##M##_##W##_i1, #M " $1,%%" RB ",%%" RA) INK(k_##M##_##W##_i2, #M " $2,%%" RB ",%%" RA) \
    INK(k_##M##_##W##_i7, #M " $7,%%" RB ",%%" RA) INK(k_##M##_##W##_i15, #M " $15,%%" RB ",%%" RA) INK(k_##M##_##W##_i31, #M " $31,%%" RB ",%%" RA) INK(k_##M##_##W##_mi5, #M " $5,%%" RB ",3(%1)")
#define DSKALL(M) DSK(M, 16, "ax", "bx", "w") DSK(M, 32, "eax", "ebx", "l") DSK(M, 64, "rax", "rbx", "q")
DSKALL(shld) DSKALL(shrd)
INK(k_shld_64_i63, "shld $63,%%rbx,%%rax") INK(k_shrd_64_i63, "shrd $63,%%rbx,%%rax") INK(k_shld_64_i33, "shld $33,%%rbx,%%rax") INK(k_shrd_64_i33, "shrd $33,%%rbx,%%rax")
// mul / imul / div / idiv
#define MDK(M, W, RB, SFX) INK(k_##M##_##W##_r, #M " %%" RB) INK(k_##M##_##W##_m, #M SFX " 64(%0)")
#define MDALL(M) MDK(M, 8, "bl", "b") MDK(M, 16, "bx", "w") MDK(M, 32, "ebx", "l") MDK(M, 64, "rbx", "q")
MDALL(mul) MDALL(imul) MDALL(div) MDALL(idiv)
#define IM2(W, RA, RB, SFX) INK(k_imul2_##W##_rr, "imul %%" RB ",%%" RA) INK(k_imul2_##W##_rm, "imul 64(%0),%%" RA) INK(k_imul2_##W##_aa, "imul %%" RA ",%%" RA) \
    INK(k_imul3_##W##_i7f, "imul $0x7f,%%" RB ",%%" RA) INK(k_imul3_##W##_im3, "imul $-3,64(%0),%%" RA) INK(k_imul3_##W##_i32, "imul $0x12345,%%" RB ",%%" RA) INK(k_imul3_##W##_ineg, "imul $-0x1000,%%" RB ",%%" RA)
IM2(16, "ax", "bx", "w") IM2(32, "eax", "ebx", "l") IM2(64, "rax", "rbx", "q")
// bit scans and counts
#define BSK(M, W, RA, RB) INK(k_##M##_##W##_rr, #M " %%" RB ",%%" RA) INK(k_##M##_##W##_rm, #M " 64(%0),%%" RA) INK(k_##M##_##W##_aa, #M " %%" RA ",%%" RA)
#define BSALL(M) BSK(M, 16, "ax", "bx") BSK(M, 32, "eax", "ebx") BSK(M, 64, "rax", "rbx")
BSALL(bsf) BSALL(bsr) BSALL(tzcnt) BSALL(lzcnt) BSALL(popcnt)
// bit tests
#define BTK(M, W, RA, RB, SFX) INK(k_##M##_##W##_rr, #M " %%" RB ",%%" RA) INK(k_##M##_##W##_i0, #M " $0,%%" RA) INK(k_##M##_##W##_i5, #M " $5,%%" RA) INK(k_##M##_##W##_i15, #M " $15,%%" RB) \
    INK(k_##M##_##W##_i31, #M " $31,%%" RA) INK(k_##M##_##W##_i33, #M " $33,%%" RA) INK(k_##M##_##W##_i63, #M " $63,%%" RA) INK(k_##M##_##W##_i255, #M " $255,%%" RA) INK(k_##M##_##W##_mi, #M SFX " $13,3(%1)")
#define BTALL(M) BTK(M, 16, "ax", "bx", "w") BTK(M, 32, "eax", "ebx", "l") BTK(M, 64, "rax", "rbx", "q")
BTALL(bt) BTALL(bts) BTALL(btr) BTALL(btc)
// BMI
#define BMK3(M, W, RA, RB, RC) INK(k_##M##_##W##_rr, #M " %%" RC ",%%" RB ",%%" RA) INK(k_##M##_##W##_rm, #M " 64(%0),%%" RB ",%%" RA)
#define BMK3ALL(M) BMK3(M, 32, "eax", "ebx", "ecx") BMK3(M, 64, "rax", "rbx", "rcx")
BMK3ALL(andn)
#define BMK3M(M, W, RA, RB, RC) INK(k_##M##_##W##_rr, #M " %%" RC ",%%" RB ",%%" RA) INK(k_##M##_##W##_rm, #M " %%" RC ",64(%0),%%" RA)
#define BMK3MALL(M) BMK3M(M, 32, "eax", "ebx", "ecx") BMK3M(M, 64, "rax", "rbx", "rcx")
BMK3MALL(bextr) BMK3MALL(bzhi) BMK3MALL(shlx) BMK3MALL(shrx) BMK3MALL(sarx)
#define BMK2(M, W, RA, RB) INK(k_##M##_##W##_rr, #M " %%" RB ",%%" RA) INK(k_##M##_##W##_rm, #M " 64(%0),%%" RA)
BMK2(blsr, 32, "eax", "ebx") BMK2(blsr, 64, "rax", "rbx") BMK2(blsi, 32, "eax", "ebx") BMK2(blsi, 64, "rax", "rbx")
#define PEK(M, W, RA, RB, RC) INK(k_##M##_##W##_rr, #M " %%" RC ",%%" RB ",%%" RA)
PEK(pext, 32, "eax", "ebx", "ecx") PEK(pext, 64, "rax", "rbx", "rcx") PEK(pdep, 32, "eax", "ebx", "ecx") PEK(pdep, 64, "rax", "rbx", "rcx")
INK(k_mulx_32_rr, "mulx %%ecx,%%ebx,%%eax") INK(k_mulx_64_rr, "mulx %%rcx,%%rbx,%%rax") INK(k_mulx_32_rm, "mulx 64(%0),%%ebx,%%eax") INK(k_mulx_64_rm, "mulx 64(%0),%%rbx,%%rax")
#define RORXK(W, RA, RB) INK(k_rorx_##W##_i1, "rorx $1,%%" RB ",%%" RA) INK(k_rorx_##W##_i7, "rorx $7,%%" RB ",%%" RA) INK(k_rorx_##W##_i31, "rorx $31,%%" RB ",%%" RA) INK(k_rorx_##W##_i13, "rorx $13,64(%0),%%" RA)
RORXK(32, "eax", "ebx") INK(k_rorx_64_i1, "rorx $1,%%rbx,%%rax") INK(k_rorx_64_i7, "rorx $7,%%rbx,%%rax") INK(k_rorx_64_i63, "rorx $63,%%rbx,%%rax") INK(k_rorx_64_i13, "rorx $13,64(%0),%%rax")
INK(k_bswap_32, "bswap %%eax") INK(k_bswap_64, "bswap %%rax") INK(k_bswap_32b, "bswap %%ebx") INK(k_bswap_64b, "bswap %%rbx")
#define MBK(W, RA, SFX) INK(k_movbe_##W##_ld, "movbe 64(%0),%%" RA) INK(k_movbe_##W##_ld3, "movbe 67(%0),%%" RA) INK(k_movbe_##W##_st, "movbe %%" RA ",3(%1)") INK(k_movbe_##W##_st0, "movbe %%" RA ",(%1)")
MBK(16, "ax", "w") MBK(32, "eax", "l") MBK(64, "rax", "q")
INK(k_cbw, "cbtw") INK(k_cwde, "cwtl") INK(k_cdqe, "cltq") INK(k_cwd, "cwtd") INK(k_cdq, "cltd") INK(k_cqo, "cqto")

// generator: operands that make div / idiv succeed, counts around the widths, bextr / bzhi controls
static void fix_sh(IRec *r, const Op *o, int stream) {
    fix_int(r, o, stream);
    int pv = (int)(intptr_t)o->p, kind = pv & 0xFF, wb = (pv >> 8) & 15, grp = (pv >> 24) & 15, w = wb * 8; uint64_t m = mskb(w);
    int cnt_kernel = grp == G_SH || (grp == G_BMI && (kind == BM_SHLX || kind == BM_SHRX || kind == BM_SARX || kind == BM_BEXTR || kind == BM_BZHI)) || (grp == G_BT);
    if (cnt_kernel) {
        uint64_t cv; switch (rnd() % 8) { case 0: cv = rnd(); break; case 1: cv = 0; break; case 2: cv = (uint64_t)w; break; case 3: cv = (uint64_t)(w - 1); break; case 4: cv = (uint64_t)(w + 1); break; default: cv = rnd() % 72; }
        if (grp == G_BMI && kind == BM_BEXTR) cv = (rnd() % (uint64_t)(w + 6)) | ((rnd() % (uint64_t)(w + 6)) << 8) | (rnd() & ~0xFFFFull);
        if (grp == G_BMI && kind == BM_BZHI) cv = (rnd() % (uint64_t)(w + 6)) | (rnd() & ~0xFFull);
        st_(r->c, 8, cv);
    }
    if (grp == G_SH && (kind == SH_SHLD || kind == SH_SHRD) && wb == 2) st_(r->c, 8, ld_(r->c, 8) % 17);      // 16-bit double shifts with a count above 16 are undefined as a whole
    if (grp == G_BMI && (kind == BM_PEXT || kind == BM_PDEP || kind == BM_ANDN || kind == BM_MULX)) { if (rnd() & 1) st_(r->c, 8, (rnd() & rnd()) | (rnd() % 4 == 0 ? rnd() : 0)); }
    if (grp == G_MD && (kind == MD_DIV || kind == MD_IDIV)) {
        uint64_t d = ld_(r->b, 8) & m, q = ld_(r->a, 8) & m; int sg = kind == MD_IDIV;
        if (rnd() % 3 == 0) d = gi_el(w, 1); if (rnd() % 3 == 0) q = gi_el(w, 1);
        if (d == 0) d = 1 + (rnd() % 7); if (sg && sx_(d, w) == 0) d = 1;
        u128_t n; uint64_t rm;
        if (!sg) { uint64_t rr = rnd() % 3 == 0 ? 0 : (rnd() % d); rm = rr; n = (u128_t)q * d + rr; }
        else { int64_t sd = sx_(d, w), sq = sx_(q, w); uint64_t ad = (uint64_t)(sd < 0 ? -(s128_t)sd : (s128_t)sd), rr = rnd() % 3 == 0 ? 0 : (rnd() % ad);
            s128_t prod = (s128_t)sq * (s128_t)sd; int sgn = prod > 0 ? 1 : prod < 0 ? -1 : ((rnd() & 1) ? 1 : -1); s128_t rs = (s128_t)sgn * (s128_t)rr; n = (u128_t)(prod + rs); rm = (uint64_t)rs; }
        (void)rm; st_(r->b, 8, (ld_(r->b, 8) & ~m) | d);
        if (wb == 1) { st_(r->a, 8, (ld_(r->a, 8) & ~0xFFFFull) | ((uint64_t)n & 0xFFFF)); }
        else if (wb == 2) { st_(r->a, 8, (ld_(r->a, 8) & ~0xFFFFull) | ((uint64_t)n & 0xFFFF)); st_(r->d, 8, (ld_(r->d, 8) & ~0xFFFFull) | (uint64_t)((n >> 16) & 0xFFFF)); }
        else if (wb == 4) { st_(r->a, 8, (uint64_t)n & 0xFFFFFFFFull); st_(r->d, 8, (uint64_t)(n >> 32) & 0xFFFFFFFFull); }
        else { st_(r->a, 8, (uint64_t)n); st_(r->d, 8, (uint64_t)(n >> 64)); }
        if (stream == 6 && 0) {}
    }
}
static Op *reg_sh(const char *name, KF k, int grp, int kind, int wb, int dst, int src, int imm, int memoff, int alias, int mem_r, uint32_t fmask) {
    Op *o = newop(name, k, m_intsh, 32, EK_I8); o->p = (void *)(intptr_t)(kind | (wb << 8) | (dst << 16) | (src << 20) | (grp << 24)); o->sz = wb; o->imm = imm; o->memoff = memoff; o->alias = alias; o->mem_r = mem_r; o->nsrc = 2; o->exh = 0;
    xall(o, fmask); o->fix = fix_sh; o->umask = um_intsh; return o;
}
#define NS_(...) (snprintf(b, sizeof b, __VA_ARGS__), nm_(b, ""))
static void reg_intsh_ops(void) {
    char b[80];
#define RSH(M, KIND, W, WB) \
    reg_sh(NS_(#M "_r" #W "_cl"), k_##M##_##W##_cl, G_SH, KIND, WB, 0, 5, 0, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_cl_b"), k_##M##_##W##_clb, G_SH, KIND, WB, 1, 5, 0, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_m" #W "_cl"), k_##M##_##W##_mcl, G_SH, KIND, WB, 2, 5, 0, 3, AL_DD, 1, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_1"), k_##M##_##W##_i1, G_SH, KIND, WB, 0, 3, 1, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_2"), k_##M##_##W##_i2, G_SH, KIND, WB, 0, 3, 2, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_7"), k_##M##_##W##_i7, G_SH, KIND, WB, 0, 3, 7, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_8"), k_##M##_##W##_i8, G_SH, KIND, WB, 0, 3, 8, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_16"), k_##M##_##W##_i16, G_SH, KIND, WB, 1, 3, 16, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_31"), k_##M##_##W##_i31, G_SH, KIND, WB, 0, 3, 31, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_33"), k_##M##_##W##_i33, G_SH, KIND, WB, 1, 3, 33, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_63"), k_##M##_##W##_i63, G_SH, KIND, WB, 0, 3, 63, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_m" #W "_5"), k_##M##_##W##_mi5, G_SH, KIND, WB, 2, 3, 5, 3, AL_DD, 1, FL_ALL); reg_sh(NS_(#M "_m" #W "_1"), k_##M##_##W##_mi1, G_SH, KIND, WB, 2, 3, 1, 0, AL_DD, 1, FL_ALL);
#define RSHALL(M, KIND) RSH(M, KIND, 8, 1) RSH(M, KIND, 16, 2) RSH(M, KIND, 32, 4) RSH(M, KIND, 64, 8)
    RSHALL(shl, SH_SHL) RSHALL(shr, SH_SHR) RSHALL(sar, SH_SAR) RSHALL(rol, SH_ROL) RSHALL(ror, SH_ROR)
#define RDS(M, KIND, W, WB) \
    reg_sh(NS_(#M "_r" #W "_cl"), k_##M##_##W##_cl, G_SH, KIND, WB, 0, 5, 0, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_m" #W "_cl"), k_##M##_##W##_mcl, G_SH, KIND, WB, 2, 5, 0, 3, AL_DD, 1, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_1"), k_##M##_##W##_i1, G_SH, KIND, WB, 0, 3, 1, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_2"), k_##M##_##W##_i2, G_SH, KIND, WB, 0, 3, 2, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_7"), k_##M##_##W##_i7, G_SH, KIND, WB, 0, 3, 7, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_15"), k_##M##_##W##_i15, G_SH, KIND, WB, 0, 3, 15, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_m" #W "_5"), k_##M##_##W##_mi5, G_SH, KIND, WB, 2, 3, 5, 3, AL_DD, 1, FL_ALL);
    RDS(shld, SH_SHLD, 16, 2) RDS(shld, SH_SHLD, 32, 4) RDS(shld, SH_SHLD, 64, 8) RDS(shrd, SH_SHRD, 16, 2) RDS(shrd, SH_SHRD, 32, 4) RDS(shrd, SH_SHRD, 64, 8)
    reg_sh("shld_r32_31", k_shld_32_i31, G_SH, SH_SHLD, 4, 0, 3, 31, 0, AL_DD, 0, FL_ALL); reg_sh("shrd_r32_31", k_shrd_32_i31, G_SH, SH_SHRD, 4, 0, 3, 31, 0, AL_DD, 0, FL_ALL);
    reg_sh("shld_r64_63", k_shld_64_i63, G_SH, SH_SHLD, 8, 0, 3, 63, 0, AL_DD, 0, FL_ALL); reg_sh("shrd_r64_63", k_shrd_64_i63, G_SH, SH_SHRD, 8, 0, 3, 63, 0, AL_DD, 0, FL_ALL);
    reg_sh("shld_r64_33", k_shld_64_i33, G_SH, SH_SHLD, 8, 0, 3, 33, 0, AL_DD, 0, FL_ALL); reg_sh("shrd_r64_33", k_shrd_64_i33, G_SH, SH_SHRD, 8, 0, 3, 33, 0, AL_DD, 0, FL_ALL);
#define RMD(M, KIND, W, WB) reg_sh(NS_(#M "_r" #W), k_##M##_##W##_r, G_MD, KIND, WB, 0, 1, 0, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_m" #W), k_##M##_##W##_m, G_MD, KIND, WB, 0, 2, 0, 0, AL_MEM, 0, FL_ALL);
#define RMDALL(M, KIND) RMD(M, KIND, 8, 1) RMD(M, KIND, 16, 2) RMD(M, KIND, 32, 4) RMD(M, KIND, 64, 8)
    RMDALL(mul, MD_MUL) RMDALL(imul, MD_IMUL1) RMDALL(div, MD_DIV) RMDALL(idiv, MD_IDIV)
#define RIM(W, WB) reg_sh(NS_("imul_r" #W "_r" #W), k_imul2_##W##_rr, G_MD, MD_IMUL2, WB, 0, 1, 0, 0, AL_DD, 0, FL_ALL); reg_sh(NS_("imul_r" #W "_m" #W), k_imul2_##W##_rm, G_MD, MD_IMUL2, WB, 0, 2, 0, 0, AL_MEM, 0, FL_ALL); \
    reg_sh(NS_("imul_r" #W "_same"), k_imul2_##W##_aa, G_MD, MD_IMUL2, WB, 0, 0, 0, 0, AL_DD, 0, FL_ALL); reg_sh(NS_("imul_r" #W "_r_i7f"), k_imul3_##W##_i7f, G_MD, MD_IMUL3, WB, 0, 1, 0x7f, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_("imul_r" #W "_m_im3"), k_imul3_##W##_im3, G_MD, MD_IMUL3, WB, 0, 2, -3, 0, AL_MEM, 0, FL_ALL); reg_sh(NS_("imul_r" #W "_r_i12345"), k_imul3_##W##_i32, G_MD, MD_IMUL3, WB, 0, 1, 0x12345, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_("imul_r" #W "_r_ineg1000"), k_imul3_##W##_ineg, G_MD, MD_IMUL3, WB, 0, 1, -0x1000, 0, AL_DD, 0, FL_ALL);
    RIM(16, 2) RIM(32, 4) RIM(64, 8)
#define RBS(M, KIND, W, WB) reg_sh(NS_(#M "_r" #W "_r" #W), k_##M##_##W##_rr, G_BS, KIND, WB, 0, 1, 0, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_m" #W), k_##M##_##W##_rm, G_BS, KIND, WB, 0, 2, 0, 0, AL_MEM, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_same"), k_##M##_##W##_aa, G_BS, KIND, WB, 0, 0, 0, 0, AL_DD, 0, FL_ALL);
#define RBSALL(M, KIND) RBS(M, KIND, 16, 2) RBS(M, KIND, 32, 4) RBS(M, KIND, 64, 8)
    RBSALL(bsf, BS_BSF) RBSALL(bsr, BS_BSR) RBSALL(tzcnt, BS_TZCNT) RBSALL(lzcnt, BS_LZCNT) RBSALL(popcnt, BS_POPCNT)
#define RBT(M, KIND, W, WB) reg_sh(NS_(#M "_r" #W "_r" #W), k_##M##_##W##_rr, G_BT, KIND, WB, 0, 1, 0, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_0"), k_##M##_##W##_i0, G_BT, KIND, WB, 0, 3, 0, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_5"), k_##M##_##W##_i5, G_BT, KIND, WB, 0, 3, 5, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_15"), k_##M##_##W##_i15, G_BT, KIND, WB, 1, 3, 15, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_31"), k_##M##_##W##_i31, G_BT, KIND, WB, 0, 3, 31, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_33"), k_##M##_##W##_i33, G_BT, KIND, WB, 0, 3, 33, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_63"), k_##M##_##W##_i63, G_BT, KIND, WB, 0, 3, 63, 0, AL_DD, 0, FL_ALL); \
    reg_sh(NS_(#M "_r" #W "_255"), k_##M##_##W##_i255, G_BT, KIND, WB, 0, 3, 255, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_m" #W "_13"), k_##M##_##W##_mi, G_BT, KIND, WB, 2, 3, 13, 3, AL_DD, 1, FL_ALL);
#define RBTALL(M, KIND) RBT(M, KIND, 16, 2) RBT(M, KIND, 32, 4) RBT(M, KIND, 64, 8)
    RBTALL(bt, 0) RBTALL(bts, 1) RBTALL(btr, 2) RBTALL(btc, 3)
#define RBM3(M, KIND, W, WB) reg_sh(NS_(#M "_r" #W "_r" #W), k_##M##_##W##_rr, G_BMI, KIND, WB, 0, 6, 0, 0, AL_DD, 0, FL_ALL); reg_sh(NS_(#M "_r" #W "_m" #W), k_##M##_##W##_rm, G_BMI, KIND, WB, 0, 2, 0, 0, AL_MEM, 0, FL_ALL);
#define RBM3ALL(M, KIND) RBM3(M, KIND, 32, 4) RBM3(M, KIND, 64, 8)
    RBM3ALL(andn, BM_ANDN) RBM3ALL(bextr, BM_BEXTR) RBM3ALL(bzhi, BM_BZHI) RBM3ALL(blsr, BM_BLSR) RBM3ALL(blsi, BM_BLSI) RBM3ALL(shlx, BM_SHLX) RBM3ALL(shrx, BM_SHRX) RBM3ALL(sarx, BM_SARX) RBM3ALL(mulx, BM_MULX)
#define RPE(M, KIND, W, WB) reg_sh(NS_(#M "_r" #W), k_##M##_##W##_rr, G_BMI, KIND, WB, 0, 6, 0, 0, AL_DD, 0, FL_ALL);
    RPE(pext, BM_PEXT, 32, 4) RPE(pext, BM_PEXT, 64, 8) RPE(pdep, BM_PDEP, 32, 4) RPE(pdep, BM_PDEP, 64, 8)
    reg_sh("rorx_r32_1", k_rorx_32_i1, G_BMI, BM_RORX, 4, 0, 3, 1, 0, AL_DD, 0, FL_ALL); reg_sh("rorx_r32_7", k_rorx_32_i7, G_BMI, BM_RORX, 4, 0, 3, 7, 0, AL_DD, 0, FL_ALL); reg_sh("rorx_r32_31", k_rorx_32_i31, G_BMI, BM_RORX, 4, 0, 3, 31, 0, AL_DD, 0, FL_ALL);
    reg_sh("rorx_r32_m32_13", k_rorx_32_i13, G_BMI, BM_RORX, 4, 0, 2, 13, 0, AL_MEM, 0, FL_ALL);
    reg_sh("rorx_r64_1", k_rorx_64_i1, G_BMI, BM_RORX, 8, 0, 3, 1, 0, AL_DD, 0, FL_ALL); reg_sh("rorx_r64_7", k_rorx_64_i7, G_BMI, BM_RORX, 8, 0, 3, 7, 0, AL_DD, 0, FL_ALL); reg_sh("rorx_r64_63", k_rorx_64_i63, G_BMI, BM_RORX, 8, 0, 3, 63, 0, AL_DD, 0, FL_ALL);
    reg_sh("rorx_r64_m64_13", k_rorx_64_i13, G_BMI, BM_RORX, 8, 0, 2, 13, 0, AL_MEM, 0, FL_ALL);
    reg_sh("bswap_r32", k_bswap_32, G_BMI, BM_BSWAP, 4, 0, 4, 0, 0, AL_DD, 0, FL_ALL); reg_sh("bswap_r64", k_bswap_64, G_BMI, BM_BSWAP, 8, 0, 4, 0, 0, AL_DD, 0, FL_ALL);
    reg_sh("bswap_r32_b", k_bswap_32b, G_BMI, BM_BSWAP, 4, 1, 4, 0, 0, AL_DD, 0, FL_ALL); reg_sh("bswap_r64_b", k_bswap_64b, G_BMI, BM_BSWAP, 8, 1, 4, 0, 0, AL_DD, 0, FL_ALL);
#define RMB(W, WB) reg_sh(NS_("movbe_r" #W "_m" #W), k_movbe_##W##_ld, G_BMI, BM_MOVBE_LD, WB, 0, 2, 0, 0, AL_MEM, 0, FL_ALL); reg_sh(NS_("movbe_r" #W "_m" #W "_o3"), k_movbe_##W##_ld3, G_BMI, BM_MOVBE_LD, WB, 0, 2, 0, 3, AL_MEM, 0, FL_ALL); \
    reg_sh(NS_("movbe_m" #W "_r" #W "_o3"), k_movbe_##W##_st, G_BMI, BM_MOVBE_ST, WB, 2, 0, 0, 3, AL_DD, 1, FL_ALL); reg_sh(NS_("movbe_m" #W "_r" #W), k_movbe_##W##_st0, G_BMI, BM_MOVBE_ST, WB, 2, 0, 0, 0, AL_DD, 1, FL_ALL);
    RMB(16, 2) RMB(32, 4) RMB(64, 8)
    reg_sh("cbw", k_cbw, G_CVT, 0, 2, 0, 4, 0, 0, AL_DD, 0, FL_ALL); reg_sh("cwde", k_cwde, G_CVT, 1, 4, 0, 4, 0, 0, AL_DD, 0, FL_ALL); reg_sh("cdqe", k_cdqe, G_CVT, 2, 8, 0, 4, 0, 0, AL_DD, 0, FL_ALL);
    reg_sh("cwd", k_cwd, G_CVT, 3, 2, 0, 4, 0, 0, AL_DD, 0, FL_ALL); reg_sh("cdq", k_cdq, G_CVT, 4, 4, 0, 4, 0, 0, AL_DD, 0, FL_ALL); reg_sh("cqo", k_cqo, G_CVT, 5, 8, 0, 4, 0, 0, AL_DD, 0, FL_ALL);
}
#endif
