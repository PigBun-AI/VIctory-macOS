// Software model of the AVX bitwise logic instructions (vand/vandn/vor/vxor ps/pd and vpand/vpandn/vpor/vpxor): dst = a OP b, with ANDN = ~a & b.
// Portable C (validated on arm64 by tests/validate-logic.c).
#ifndef ISA_MODEL_LOGIC_H
#define ISA_MODEL_LOGIC_H
#include "isa-types.h"
#ifndef MODEL_NEG
#define MODEL_NEG 0
#endif
enum { LG_AND, LG_ANDN, LG_OR, LG_XOR };
static void m_logic(const Ctx *c, const IRec *r, Out *o) {
    int k = (int)(intptr_t)c->op->p; const uint8_t *b = c->op->alias == AL_MEM ? msrc(r, c->op) : r->b; memset(o->r, 0, 32);
    for (int i = 0; i < c->w; i++) {
        uint8_t x = r->a[i], y = b[i];
        if (MODEL_NEG && k == LG_ANDN) o->r[i] = x & ~y;           // negative control: operands of ANDN swapped
        else o->r[i] = k == LG_AND ? (x & y) : k == LG_ANDN ? (uint8_t)(~x & y) : k == LG_OR ? (x | y) : (x ^ y);
    }
}
#endif
