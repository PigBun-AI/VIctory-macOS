// Software models of the AVX move family (see isa-move.h). Portable C: validated on arm64 by tests/validate-move.c.
#ifndef ISA_MODEL_MOVE_H
#define ISA_MODEL_MOVE_H
#include "isa-types.h"
static const uint8_t *src2f(const Op *o, const IRec *r) { return o->alias == AL_DA ? r->a : r->b; }
static void m_copy(const Ctx *c, const IRec *r, Out *o) { memset(o->r, 0, 32); memcpy(o->r, src2f(c->op, r), c->w); }
static void m_load(const Ctx *c, const IRec *r, Out *o) { memset(o->r, 0, 32); memcpy(o->r, msrc(r, c->op), c->op->sz); }
static void m_store(const Ctx *c, const IRec *r, Out *o) { memcpy((uint8_t *)o + c->op->memoff, r->a + c->op->p2, c->op->sz); }      // o->r || o->x is the pre-filled 64-byte image
static void m_movss(const Ctx *c, const IRec *r, Out *o) { int n = c->op->sz; memset(o->r, 0, 32); memcpy(o->r, r->a, 16); memcpy(o->r, r->b, (size_t)n); }       // vmovss/vmovsd x,x,x: low element from src2, the rest of the low 128 bits from src1
static void m_lhps(const Ctx *c, const IRec *r, Out *o) { memset(o->r, 0, 32); memcpy(o->r, r->a, 8); memcpy(o->r + 8, r->b, 8); }
static void m_hlps(const Ctx *c, const IRec *r, Out *o) { memset(o->r, 0, 32); memcpy(o->r, r->b + 8, 8); memcpy(o->r + 8, r->a + 8, 8); }
static void m_lps_load(const Ctx *c, const IRec *r, Out *o) { memset(o->r, 0, 32); memcpy(o->r, r->a, 16); memcpy(o->r, msrc(r, c->op), 8); }          // vmovlps/vmovlpd x,x,m64: low qword from memory, high from src1
static void m_hps_load(const Ctx *c, const IRec *r, Out *o) { memset(o->r, 0, 32); memcpy(o->r, r->a, 16); memcpy(o->r + 8, msrc(r, c->op), 8); }
static void m_dup(const Ctx *c, const IRec *r, Out *o, const uint8_t *s) {             // movshdup (p=3: elements 1,1,3,3), movsldup (p=2: 0,0,2,2), movddup (p=1)
    int kind = (int)(intptr_t)c->op->p; memset(o->r, 0, 32);
    for (int l = 0; l < c->w; l += 16) {
        if (kind == 3) { for (int i = 0; i < 4; i++) memcpy(o->r + l + i * 4, s + l + ((i | 1) * 4), 4); }
        else if (kind == 2) { for (int i = 0; i < 4; i++) memcpy(o->r + l + i * 4, s + l + ((i & 2) * 4), 4); }
        else { memcpy(o->r + l, s + l, 8); memcpy(o->r + l + 8, s + l, 8); }
    }
}
static void m_dup_r(const Ctx *c, const IRec *r, Out *o) { m_dup(c, r, o, src2f(c->op, r)); }
static void m_dup_m(const Ctx *c, const IRec *r, Out *o) {
    int kind = (int)(intptr_t)c->op->p; if (kind == 1 && c->w == 16) { const uint8_t *s = msrc(r, c->op); memset(o->r, 0, 32); memcpy(o->r, s, 8); memcpy(o->r + 8, s, 8); return; }      // vmovddup x,m64 reads 8 bytes
    m_dup(c, r, o, msrc(r, c->op));
}
static void m_bcast(const Ctx *c, const IRec *r, Out *o) {                         // broadcast the element (sz bytes) of src2 / memory over the whole vector; f128/i128 broadcast 16 bytes
    int sz = c->op->sz; const uint8_t *s = c->op->alias == AL_MEM ? msrc(r, c->op) : src2f(c->op, r);
    memset(o->r, 0, 32); uint8_t t[16]; memcpy(t, s, (size_t)sz); for (int i = 0; i < c->w; i += sz) memcpy(o->r + i, t, (size_t)sz);
}
static void m_movd_to_x(const Ctx *c, const IRec *r, Out *o) { memset(o->r, 0, 32); memcpy(o->r, src2f(c->op, r), (size_t)c->op->sz); }                   // vmovd x,r32 / vmovq x,r64 (GPR loaded from the b field)
static void m_movd_from_x(const Ctx *c, const IRec *r, Out *o) { o->x[0] = ld_(r->b, c->op->sz); }                                         // vmovd r32,x (zero-extended into the 64-bit register) / vmovq r64,x
static void m_movmskps(const Ctx *c, const IRec *r, Out *o) { uint32_t m = 0; int n = c->w / 4; for (int i = 0; i < n; i++) m |= (uint32_t)(r->b[i * 4 + 3] >> 7) << i; o->x[0] = m; }
static void m_movmskpd(const Ctx *c, const IRec *r, Out *o) { uint32_t m = 0; int n = c->w / 8; for (int i = 0; i < n; i++) m |= (uint32_t)(r->b[i * 8 + 7] >> 7) << i; o->x[0] = m; }
static void m_pmovmskb(const Ctx *c, const IRec *r, Out *o) { uint32_t m = 0; for (int i = 0; i < c->w; i++) m |= (uint32_t)(r->b[i] >> 7) << i; o->x[0] = m; }

#endif
