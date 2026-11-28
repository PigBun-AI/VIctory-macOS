#ifndef ISA_MODEL_MASKED_H
#define ISA_MODEL_MASKED_H
#include "isa-model-shuf.h"
// op->p: es | store << 8
static void m_mask(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, es = pv & 15, st = (pv >> 8) & 1, w = c->w;
    if (!st) { const uint8_t *src = msrc(r, c->op); memset(o->r, 0, 32); for (int i = 0; i < w / es; i++) if ((el_(r->a, i, es) >> (es * 8 - 1)) & 1) se_(o->r, i, es, el_(src, i, es)); return; }
    for (int i = 0; i < w / es; i++) if ((el_(r->a, i, es) >> (es * 8 - 1)) & 1) se_(o->r + c->op->memoff, i, es, el_(r->b, i, es));
}
#endif
