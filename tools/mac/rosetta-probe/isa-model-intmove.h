// Software model of the x86 integer moves and conditionals: mov (all widths, partial-register merging, 32-bit zero-extension), movzx, movsx, movsxd, xchg,
// cmovcc (16 conditions, 16/32/64 bit, including the 32-bit "false condition still zero-extends"), setcc, lea (base/index/scale/disp, 16/32/64-bit result,
// 32-bit address size), jcc / jrcxz / jecxz (taken or not) and cdq/cqo/cwde/cdqe-style sign extensions. None of them changes the flags.
// Portable C (validated on arm64 by tests/validate-intmove.c).
#ifndef ISA_MODEL_INTMOVE_H
#define ISA_MODEL_INTMOVE_H
#include "isa-model-int.h"
enum { MV_MOV, MV_MOVZX, MV_MOVSX, MV_XCHG, MV_CMOV, MV_SETCC, MV_LEA, MV_JCC, MV_JRCXZ, MV_JECXZ };
// x86 condition codes 0..15: o no b ae e ne be a s ns p np l ge le g
static int cc_true(int cc, uint32_t f) {
    int cf = !!(f & FL_CF), pf = !!(f & FL_PF), zf = !!(f & FL_ZF), sf = !!(f & FL_SF), of = !!(f & FL_OF), r = 0;
    switch (cc >> 1) { case 0: r = of; break; case 1: r = cf; break; case 2: r = zf; break; case 3: r = cf | zf; break; case 4: r = sf; break; case 5: r = pf; break; case 6: r = sf != of; break; default: r = zf | (sf != of); }
    if (MODEL_NEG && (cc >> 1) == 3) r = cf & zf;      // negative control: BE/A as an AND instead of an OR
    return (cc & 1) ? !r : r;
}
static inline uint64_t put_reg(uint64_t old, int wb, uint64_t v) { return wb == 8 ? v : wb == 4 ? (v & 0xFFFFFFFFull) : (old & ~mskb(wb * 8)) | (v & mskb(wb * 8)); }
// op->p: kind | wb << 8 | swb << 12 | dst << 16 | src << 20 | cc << 24.  dst: 0 rax, 1 rbx, 2 out.r + memoff, 3 rdx.  src: 0 rax, 1 rbx, 2 record b field + memoff, 3 immediate (op->imm, or the 64-bit immediate in c[16..23] when swb == 8 && kind == MV_MOV).
// lea: swb = scale (1, 2, 4, 8; 0 = no index), dst 3 (rdx), imm = displacement, cc: bit 0 base present, bit 1 address size 32.
static void m_intmove(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, kind = pv & 0xFF, wb = (pv >> 8) & 15, swb = (pv >> 12) & 15, dst = (pv >> 16) & 15, src = (pv >> 20) & 15, cc = (pv >> 24) & 15;
    uint64_t rax = ld_(r->a, 8), rbx = ld_(r->b, 8), rdx = ld_(r->d, 8), rcx = ld_(r->c, 8); uint32_t fin = (uint32_t)ld_(r->c + 8, 8) & FL_ALL;
    uint64_t old = dst == 0 ? rax : dst == 1 ? rbx : dst == 3 ? rdx : ld_(r->d + c->op->memoff, wb);
    int sw = swb ? swb : wb; uint64_t s = src == 0 ? rax : src == 1 ? rbx : src == 2 ? ld_(msrc(r, c->op), sw) : (uint64_t)(int64_t)c->op->imm; s &= mskb(sw * 8);
    uint64_t v = 0; int write = 1;
    switch (kind) {
    case MV_MOV: v = src == 3 && swb == 8 ? 0x8123456789abcdefull : s; break;
    case MV_MOVZX: v = s; break;
    case MV_MOVSX: v = (uint64_t)sx_(s, sw * 8); break;
    case MV_XCHG: { uint64_t sold = src == 0 ? rax : src == 1 ? rbx : ld_(msrc(r, c->op), wb); sold &= mskb(wb * 8);
        // exchange the destination (reg or memory) with the source register (a or b)
        uint64_t dv = old & mskb(wb * 8);
        if (dst == 2) st_(o->r + c->op->memoff, wb, sold); else if (dst == 0) rax = put_reg(rax, wb, sold); else rbx = put_reg(rbx, wb, sold);
        if (src == 0) rax = put_reg(rax, wb, dv); else if (src == 1) rbx = put_reg(rbx, wb, dv);
        write = 0; break; }
    case MV_CMOV: if (cc_true(cc, fin)) v = s; else { v = old; } break;
    case MV_SETCC: v = cc_true(cc, fin); break;
    case MV_LEA: { int a32 = (cc >> 1) & 1, base = cc & 1; uint64_t b0 = base ? (a32 ? (rax & 0xFFFFFFFFull) : rax) : 0, i0 = swb ? (a32 ? (rbx & 0xFFFFFFFFull) : rbx) * (uint64_t)swb : 0;
        uint64_t ea = b0 + i0 + (uint64_t)(int64_t)c->op->imm; if (a32) ea &= 0xFFFFFFFFull; v = ea; break; }
    }
    if (write) { if (dst == 2) st_(o->r + c->op->memoff, wb, v); else { uint64_t nv = put_reg(old, wb, v); if (dst == 0) rax = nv; else if (dst == 1) rbx = nv; else rdx = nv; } }
    o->x[0] = rax; o->x[1] = fin; o->x[2] = rdx; o->x[3] = rbx;
}
// jcc: x[2] = 1 when taken; jrcxz / jecxz test rcx / ecx (c[0..7])
static void m_jcc(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, kind = pv & 0xFF, cc = (pv >> 24) & 15; uint64_t rcx = ld_(r->c, 8); uint32_t fin = (uint32_t)ld_(r->c + 8, 8) & FL_ALL;
    int taken = kind == MV_JCC ? cc_true(cc, fin) : kind == MV_JRCXZ ? rcx == 0 : (rcx & 0xFFFFFFFFull) == 0;
    o->x[0] = ld_(r->a, 8); o->x[1] = fin; o->x[2] = (uint64_t)taken; o->x[3] = ld_(r->b, 8);
}
#endif
