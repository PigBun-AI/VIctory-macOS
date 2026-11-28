// Software model of the AVX shuffle / permute / unpack / pack / blend / horizontal / insert / extract instructions, written from the instruction set reference.
// Portable C (cross-checked on arm64 by tests/validate-shuf.c where an independent NEON formulation exists).
#ifndef ISA_MODEL_SHUF_H
#define ISA_MODEL_SHUF_H
#include "isa-model-fp.h"
#include "isa-model-int.h"
enum { S_SHUFPS, S_SHUFPD, S_UNPCK, S_PSHUFD, S_PSHUFHW, S_PSHUFLW, S_PERMILPS_I, S_PERMILPD_I, S_PERMILPS_V, S_PERMILPD_V, S_PSHUFB, S_PERMQ, S_PERMD, S_PERM2X128, S_INS128, S_EXT128,
       S_INSERTPS, S_BLENDI, S_BLENDV, S_PACK, S_HADD, S_PHADD, S_PALIGNR, S_PINSR, S_PEXTR };
// op->p: kind | es << 8 | opt << 16   (es: element bytes, opt: kind specific)
static inline uint64_t el_(const uint8_t *p, int i, int es) { return ld_(p + (size_t)i * es, es); }
static inline void se_(uint8_t *p, int i, int es, uint64_t v) { st_(p + (size_t)i * es, es, v); }
static int64_t sat_s(int64_t v, int bits) { int64_t hi = (1ll << (bits - 1)) - 1, lo = -hi - 1; return v > hi ? hi : v < lo ? lo : v; }
static int64_t sat_u(int64_t v, int bits) { int64_t hi = (1ll << bits) - 1; return v > hi ? hi : v < 0 ? 0 : v; }
static void m_shuf(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, kind = pv & 0xFF, es = (pv >> 8) & 15, opt = (pv >> 16) & 255, w = c->w, nl = w / 16, imm = c->imm & 0xFF;
    const uint8_t *a = r->a, *b = c->op->alias == AL_MEM ? msrc(r, c->op) : r->b; uint8_t *d = o->r; if (!c->op->mem_r) memset(d, 0, 32);
    for (int L = 0; L < nl; L++) {
        const uint8_t *la = a + 16 * L, *lb = b + 16 * L; uint8_t *ld = d + 16 * L; int n = 16 / (es ? es : 1);
        switch (kind) {
        case S_SHUFPS: for (int i = 0; i < 2; i++) { se_(ld, i, 4, el_(la, (imm >> (2 * i)) & 3, 4)); se_(ld, i + 2, 4, el_(lb, (imm >> (2 * i + 4)) & 3, 4)); } break;
        case S_SHUFPD: se_(ld, 0, 8, el_(la, (imm >> (2 * L)) & 1, 8)); se_(ld, 1, 8, el_(lb, (imm >> (2 * L + 1)) & 1, 8)); break;
        case S_UNPCK: for (int i = 0; i < n / 2; i++) { int s = opt ? n / 2 + i : i; se_(ld, 2 * i, es, el_(la, s, es)); se_(ld, 2 * i + 1, es, el_(lb, s, es)); } break;
        case S_PSHUFD: case S_PERMILPS_I: for (int i = 0; i < 4; i++) se_(ld, i, 4, el_(lb, (imm >> (2 * i)) & 3, 4)); break;
        case S_PSHUFHW: memcpy(ld, lb, 8); for (int i = 0; i < 4; i++) se_(ld + 8, i, 2, el_(lb + 8, (imm >> (2 * i)) & 3, 2)); break;
        case S_PSHUFLW: memcpy(ld + 8, lb + 8, 8); for (int i = 0; i < 4; i++) se_(ld, i, 2, el_(lb, (imm >> (2 * i)) & 3, 2)); break;
        case S_PERMILPD_I: for (int j = 0; j < 2; j++) se_(ld, j, 8, el_(lb, (imm >> (2 * L + j)) & 1, 8)); break;
        case S_PERMILPS_V: for (int i = 0; i < 4; i++) se_(ld, i, 4, el_(la, (int)(el_(lb, i, 4) & 3), 4)); break;
        case S_PERMILPD_V: for (int j = 0; j < 2; j++) se_(ld, j, 8, el_(la, (int)((el_(lb, j, 8) >> 1) & 1), 8)); break;
        case S_PSHUFB: for (int i = 0; i < 16; i++) ld[i] = (lb[i] & 0x80) ? 0 : la[lb[i] & 15]; break;
        case S_BLENDI: { int bits = 0; for (int i = 0; i < n; i++) { int bit = es == 2 ? (imm >> i) & 1 : (imm >> (L * n + i)) & 1; se_(ld, i, es, bit ? el_(lb, i, es) : el_(la, i, es)); } (void)bits; break; }
        case S_BLENDV: { const uint8_t *lc = r->c + 16 * L; for (int i = 0; i < n; i++) se_(ld, i, es, (el_(lc, i, es) >> (es * 8 - 1)) & 1 ? el_(lb, i, es) : el_(la, i, es)); break; }
        case S_PACK: {       // opt: 0 ssdw->w 1 sswb 2 uswb 3 usdw ; es = destination element size
            int ss = es * 2, nn = 16 / ss; for (int i = 0; i < nn; i++) for (int h = 0; h < 2; h++) { int64_t v = sx_(el_(h ? lb : la, i, ss), ss * 8); int64_t s = (opt == 0 || opt == 1) ? sat_s(v, es * 8) : sat_u(v, es * 8); se_(ld, h * nn + i, es, (uint64_t)s); } break; }
        case S_HADD: { Fmt f = es == 8 ? F64 : F32; Fpc fc = fpc_of(c->mx); int k = opt ? FP_SUB : FP_ADD; int h = n / 2;
            for (int i = 0; i < h; i++) { se_(ld, i, es, fp_op(k, f, el_(la, 2 * i, es), el_(la, 2 * i + 1, es), fc)); se_(ld, h + i, es, fp_op(k, f, el_(lb, 2 * i, es), el_(lb, 2 * i + 1, es), fc)); } break; }
        case S_PHADD: { int h = n / 2; for (int i = 0; i < h; i++) { for (int q = 0; q < 2; q++) { const uint8_t *s = q ? lb : la; int64_t x = sx_(el_(s, 2 * i, es), es * 8), y = sx_(el_(s, 2 * i + 1, es), es * 8), v;
                v = (opt & 1) ? x - y : x + y; if (opt >= 2) v = sat_s(v, es * 8); se_(ld, q * h + i, es, (uint64_t)v & mskb(es * 8)); } } break; }
        case S_PALIGNR: { uint8_t t[32]; memcpy(t, lb, 16); memcpy(t + 16, la, 16); for (int i = 0; i < 16; i++) ld[i] = i + imm < 32 ? t[i + imm] : 0; break; }
        }
    }
    switch (kind) {
    case S_PERMQ: for (int i = 0; i < 4; i++) se_(d, i, 8, el_(b, (imm >> (2 * i)) & 3, 8)); break;
    case S_PERMD: for (int i = 0; i < 8; i++) se_(d, i, 4, el_(b, (int)(el_(a, i, 4) & 7), 4)); break;
    case S_PERM2X128: for (int h = 0; h < 2; h++) { int sel = (imm >> (4 * h)) & 15; if (!(sel & 8)) memcpy(d + 16 * h, ((sel & 2) ? b : a) + 16 * (sel & 1), 16); } break;
    case S_INS128: memcpy(d, a, 32); memcpy(d + 16 * (imm & 1), b, 16); break;
    case S_EXT128: if (c->op->mem_r) memcpy(d + c->op->memoff, b + 16 * (imm & 1), 16); else memcpy(d, b + 16 * (imm & 1), 16); break;
    case S_INSERTPS: { int cs = c->op->alias == AL_MEM ? 0 : (imm >> 6) & 3, cd = (imm >> 4) & 3, zm = imm & 15; memcpy(d, a, 16); se_(d, cd, 4, el_(b, cs, 4)); for (int i = 0; i < 4; i++) if ((zm >> i) & 1) se_(d, i, 4, 0); break; }
    case S_PINSR: { memcpy(d, a, 16); se_(d, imm & (16 / es - 1), es, ld_(b, es)); break; }
    case S_PEXTR: { uint64_t v = el_(b, imm & (16 / es - 1), es); if (c->op->mem_r) st_(d + c->op->memoff, es, v); else o->x[0] = v; break; }
    }
    if (MODEL_NEG && kind == S_PSHUFB) { if (r->b[0] & 0x80) d[0] = a[0]; }
}
#endif
