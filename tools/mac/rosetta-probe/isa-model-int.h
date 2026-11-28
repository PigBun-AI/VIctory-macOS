// Software model of the x86 integer ALU instructions: add sub adc sbb and or xor cmp test inc dec neg not, widths 8/16/32/64, with the six arithmetic flags
// (CF PF AF ZF SF OF) bit by bit. Flags the SDM calls undefined (AF after and/or/xor/test) are excluded from the comparison by the op's flag mask.
// Portable C (validated on arm64 by tests/validate-int.c).
#ifndef ISA_MODEL_INT_H
#define ISA_MODEL_INT_H
#include "isa-types.h"
#ifndef MODEL_NEG
#define MODEL_NEG 0
#endif
enum { IA_ADD, IA_SUB, IA_ADC, IA_SBB, IA_AND, IA_OR, IA_XOR, IA_CMP, IA_TEST, IA_INC, IA_DEC, IA_NEG, IA_NOT };
enum { FL_CF = 1, FL_PF = 4, FL_AF = 0x10, FL_ZF = 0x40, FL_SF = 0x80, FL_OF = 0x800, FL_ALL = 0x8D5 };
typedef struct { uint64_t r; uint32_t f; } AluR;
static inline uint32_t fl_szp(uint64_t r, int bits) {
    uint32_t f = 0; if (r == 0) f |= FL_ZF; if ((r >> (bits - 1)) & 1) f |= FL_SF;
    uint32_t p = (uint32_t)(r & 0xFF); p ^= p >> 4; p ^= p >> 2; p ^= p >> 1; if (!(p & 1)) f |= FL_PF; return f;
}
// a op b with carry-in cf (in fl_in: the initial flags; inc/dec/not keep what they do not write)
static AluR alu(int kind, int wb, uint64_t a, uint64_t b, uint32_t fl_in) {
    int bits = wb * 8; uint64_t m = mskb(bits), top = 1ull << (bits - 1); a &= m; b &= m; uint64_t r = 0; uint32_t f = 0, cf = fl_in & FL_CF;
    switch (kind) {
    case IA_ADD: case IA_ADC: case IA_INC: {
        uint64_t c = kind == IA_ADC ? cf : 0; if (kind == IA_INC) b = 1; r = (a + b + c) & m;
        if (r < a || (c && r == a)) f |= FL_CF; if (((a ^ r) & (b ^ r)) & top) f |= FL_OF; if ((a ^ b ^ r) & 0x10) f |= FL_AF;
        if (kind == IA_INC) f = (f & ~(uint32_t)FL_CF) | cf; break; }
    case IA_SUB: case IA_SBB: case IA_CMP: case IA_DEC: {
        uint64_t c = kind == IA_SBB ? cf : 0; if (kind == IA_DEC) b = 1; r = (a - b - c) & m;
        if (a < b || (c && a == b)) f |= FL_CF; if (((a ^ b) & (a ^ r)) & top) f |= FL_OF; if ((a ^ b ^ r) & 0x10) f |= FL_AF;
        if (kind == IA_DEC) f = (f & ~(uint32_t)FL_CF) | cf; break; }
    case IA_NEG: r = (0 - a) & m; if (a) f |= FL_CF; if (a == top) f |= FL_OF; if ((a ^ r) & 0x10) f |= FL_AF; break;
    case IA_AND: case IA_TEST: r = a & b; break;
    case IA_OR: r = a | b; break;
    case IA_XOR: r = a ^ b; break;
    case IA_NOT: { AluR o = {~a & m, fl_in & FL_ALL}; return o; }
    }
    if (MODEL_NEG && kind == IA_SUB) f ^= FL_CF;      // negative control: Arm-style carry (C = no borrow) instead of x86 CF = borrow
    f |= fl_szp(r, bits); AluR o = {r, f}; return o;
}
// op->p: kind | wb << 8 | dst << 16 | src << 20.   dst: 0 rax, 1 rbx, 2 memory (out.r + memoff, pre-filled with d).   src: 0 rax, 1 rbx, 2 memory (record b field + memoff), 3 immediate (op->imm), 4 none
static void m_alu(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, kind = pv & 0xFF, wb = (pv >> 8) & 15, dst = (pv >> 16) & 15, src = (pv >> 20) & 15; uint64_t m = mskb(wb * 8);
    uint64_t rax = ld_(r->a, 8), rbx = ld_(r->b, 8), rdx = ld_(r->d, 8); uint32_t fin = (uint32_t)ld_(r->c + 8, 8) & FL_ALL;
    uint64_t x = dst == 0 ? rax : dst == 1 ? rbx : ld_(r->d + c->op->memoff, wb);
    uint64_t y = src == 0 ? rax : src == 1 ? rbx : src == 2 ? ld_(msrc(r, c->op), wb) : src == 3 ? (uint64_t)(int64_t)c->op->imm : 0;
    AluR res = alu(kind, wb, x, y, fin); int nowrite = kind == IA_CMP || kind == IA_TEST;
    if (!nowrite) {
        if (dst == 2) st_(o->r + c->op->memoff, wb, res.r);
        else { uint64_t old = dst == 0 ? rax : rbx, nv = wb == 8 ? res.r : wb == 4 ? res.r : (old & ~m) | res.r; if (dst == 0) rax = nv; else rbx = nv; }
    }
    o->x[0] = rax; o->x[1] = res.f; o->x[2] = rdx; o->x[3] = rbx;
}
#endif
