// Software model of the x86 floating point compares: vcmpps/pd/ss/sd with the 32 AVX predicates (all-ones / zero result masks, scalar forms keep the upper bits of
// src1) and (v)ucomiss/sd, (v)comiss/sd (ZF PF CF, OF = SF = AF = 0). DAZ makes denormal operands compare as zero. Portable C (validated by tests/validate-fp.c on arm64).
#ifndef ISA_MODEL_CMP_H
#define ISA_MODEL_CMP_H
#include "isa-model-fp.h"
#include "isa-model-int.h"
// relation: 0 less, 1 equal, 2 greater, 3 unordered
static int fp_rel(Fmt f, uint64_t a, uint64_t b, int daz) {
    if (daz) { a = sf_daz(f, a); b = sf_daz(f, b); }
    if (sf_isnan(f, a) || sf_isnan(f, b)) return 3;
    if (fp_lt(f, a, b)) return 0; if (fp_lt(f, b, a)) return 2; return 1;
}
// predicate 0..31 (the signalling variants only differ in the exception flags, not in the result)
static int cmp_pred(int pred, int rel) {
    int lt = rel == 0, eq = rel == 1, gt = rel == 2, un = rel == 3, r = 0;
    switch (pred & 15) {
    case 0: r = eq; break; case 1: r = lt; break; case 2: r = lt | eq; break; case 3: r = un; break;
    case 4: r = un | lt | gt; break; case 5: r = un | eq | gt; break; case 6: r = un | gt; break; case 7: r = !un; break;
    case 8: r = eq | un; break; case 9: r = un | lt; break; case 10: r = un | lt | eq; break; case 11: r = 0; break;
    case 12: r = lt | gt; break; case 13: r = gt | eq; break; case 14: r = gt; break; default: r = 1; break;
    }
    if (MODEL_NEG && (pred & 15) == 4) r = lt | gt;      // negative control: NEQ treated as an ordered compare (Arm FCMEQ + NOT without the NaN case)
    return r;
}
// op->p: pred | dbl << 8 | scalar << 16 | kind << 20 (0 vcmp, 1 ucomi/comi)
static void m_vcmp(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, pred = pv & 0xFF, dbl = (pv >> 8) & 1, sc = (pv >> 16) & 1; Fmt f = dbl ? F64 : F32; int sz = dbl ? 8 : 4; Fpc fc = fpc_of(c->mx);
    const uint8_t *a = r->a, *b = c->op->alias == AL_MEM ? msrc(r, c->op) : r->b; memset(o->r, 0, 32);
    if (sc) { memcpy(o->r, a, 16); st_(o->r, sz, cmp_pred(pred, fp_rel(f, ld_(a, sz), ld_(b, sz), fc.daz)) ? mskb(sz * 8) : 0); return; }
    for (int i = 0; i < c->w / sz; i++) st_(o->r + i * sz, sz, cmp_pred(pred, fp_rel(f, ld_(a + i * sz, sz), ld_(b + i * sz, sz), fc.daz)) ? mskb(sz * 8) : 0);
}
static void m_ucomi(const Ctx *c, const IRec *r, Out *o) {
    int dbl = (int)(intptr_t)c->op->p & 1; Fmt f = dbl ? F64 : F32; int sz = dbl ? 8 : 4; Fpc fc = fpc_of(c->mx);
    const uint8_t *b = c->op->alias == AL_MEM ? msrc(r, c->op) : r->b; int rel = fp_rel(f, ld_(r->a, sz), ld_(b, sz), fc.daz);
    o->x[1] = rel == 3 ? (FL_ZF | FL_PF | FL_CF) : rel == 0 ? FL_CF : rel == 1 ? FL_ZF : 0;
}
#endif
