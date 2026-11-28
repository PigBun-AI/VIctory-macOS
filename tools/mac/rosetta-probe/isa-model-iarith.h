// Software model of the AVX/AVX2 integer SIMD arithmetic, compare, min/max, multiply, shift, sign/abs, extend, test and horizontal-minimum instructions, plus vaddsubps/pd.
// Written from the instruction set reference; portable C (cross-checked on arm64 by tests/validate-iarith.c).
#ifndef ISA_MODEL_IARITH_H
#define ISA_MODEL_IARITH_H
#include "isa-model-shuf.h"
enum { II_ADD, II_SUB, II_ADDS, II_ADDUS, II_SUBS, II_SUBUS, II_MINS, II_MINU, II_MAXS, II_MAXU, II_AVG, II_CMPEQ, II_CMPGT, II_MULLO, II_MULHS, II_MULHU, II_MULDQ, II_MULUDQ, II_MADDWD, II_MADDUBSW, II_MULHRSW,
       II_SADBW, II_SIGN, II_ABS, II_MOVX, II_PTEST, II_TESTP, II_SHI, II_SHX, II_SHV, II_SHDQ, II_ADDSUB, II_HMINPOS };
enum { SH_L, SH_R, SH_A };       // shift direction (opt)
// op->p: kind | es << 8 | opt << 16 | es2 << 24  (es2: source element size of the extends)
static void m_ia(const Ctx *c, const IRec *r, Out *o) {
    int pv = (int)(intptr_t)c->op->p, kind = pv & 0xFF, es = (pv >> 8) & 15, opt = (pv >> 16) & 255, es2 = (pv >> 24) & 15, w = c->w, n = w / (es ? es : 1), bits = es * 8; int imm = c->imm & 0xFF;
    const uint8_t *a = r->a, *b = c->op->alias == AL_MEM ? msrc(r, c->op) : r->b; uint8_t *d = o->r; memset(d, 0, 32);
    uint64_t M = mskb(bits);
    if ((kind == II_ABS || kind == II_MOVX || kind == II_HMINPOS) && c->op->alias == AL_DA) b = a;
    switch (kind) {
    case II_PTEST: case II_TESTP: {
        uint32_t f = 0; int zero = 1, cf = 1; for (int i = 0; i < w; i++) { uint8_t x = a[i], y = b[i];
            if (kind == II_TESTP) { int sb = (i % es) == es - 1 ? 1 : 0; if (!sb) continue; x &= 0x80; y &= 0x80; }
            if (x & y) zero = 0; if ((uint8_t)~x & y) cf = 0; }
        if (zero) f |= FL_ZF; if (cf) f |= FL_CF; o->x[1] = f; return; }
    case II_SHDQ: for (int L = 0; L < w / 16; L++) for (int i = 0; i < 16; i++) { int s = opt == SH_L ? i - imm : i + imm; d[16 * L + i] = (imm > 15 || s < 0 || s > 15) ? 0 : b[16 * L + s]; } return;
    case II_HMINPOS: { uint16_t mn = 0xFFFF, ix = 0; for (int i = 0; i < 8; i++) { uint16_t v = (uint16_t)el_(b, i, 2); if (v < mn) { mn = v; ix = (uint16_t)i; } } se_(d, 0, 2, mn); se_(d, 1, 2, ix); return; }
    case II_MOVX: { int de = es; for (int i = 0; i < w / de; i++) { uint64_t v = el_(b, i, es2); if (opt) v = (uint64_t)sx_(v, es2 * 8); se_(d, i, de, v & mskb(de * 8)); } return; }
    case II_ABS: for (int i = 0; i < n; i++) { int64_t v = sx_(el_(b, i, es), bits); se_(d, i, es, (uint64_t)(v < 0 ? -v : v) & M); } return;
    case II_SHI: for (int i = 0; i < n; i++) { uint64_t v = el_(b, i, es), cnt = (uint64_t)imm; uint64_t r2; if (opt == SH_L) r2 = cnt >= (uint64_t)bits ? 0 : (v << cnt) & M; else if (opt == SH_R) r2 = cnt >= (uint64_t)bits ? 0 : v >> cnt; else r2 = (uint64_t)(sx_(v, bits) >> (cnt >= (uint64_t)bits ? bits - 1 : cnt)) & M; se_(d, i, es, r2); } return;
    case II_SHX: { uint64_t cnt = ld_(b, 8); for (int i = 0; i < n; i++) { uint64_t v = el_(a, i, es), r2; if (opt == SH_L) r2 = cnt >= (uint64_t)bits ? 0 : (v << cnt) & M; else if (opt == SH_R) r2 = cnt >= (uint64_t)bits ? 0 : v >> cnt; else r2 = (uint64_t)(sx_(v, bits) >> (cnt >= (uint64_t)bits ? bits - 1 : cnt)) & M; se_(d, i, es, r2); } return; }
    case II_SHV: for (int i = 0; i < n; i++) { uint64_t v = el_(a, i, es), cnt = el_(b, i, es), r2; if (opt == SH_L) r2 = cnt >= (uint64_t)bits ? 0 : (v << cnt) & M; else if (opt == SH_R) r2 = cnt >= (uint64_t)bits ? 0 : v >> cnt; else r2 = (uint64_t)(sx_(v, bits) >> (cnt >= (uint64_t)bits ? bits - 1 : cnt)) & M; se_(d, i, es, r2); } return;
    case II_ADDSUB: { Fmt f = es == 8 ? F64 : F32; Fpc fc = fpc_of(c->mx); for (int i = 0; i < n; i++) se_(d, i, es, fp_op((i & 1) ? FP_ADD : FP_SUB, f, el_(a, i, es), el_(b, i, es), fc)); return; }
    case II_MULDQ: case II_MULUDQ: for (int i = 0; i < w / 8; i++) { uint64_t x = el_(a, 2 * i, 4), y = el_(b, 2 * i, 4); uint64_t p = kind == II_MULDQ ? (uint64_t)((int64_t)(int32_t)x * (int64_t)(int32_t)y) : x * y; se_(d, i, 8, p); } return;
    case II_MADDWD: for (int i = 0; i < w / 4; i++) { int64_t s = (int64_t)(int16_t)el_(a, 2 * i, 2) * (int16_t)el_(b, 2 * i, 2) + (int64_t)(int16_t)el_(a, 2 * i + 1, 2) * (int16_t)el_(b, 2 * i + 1, 2); se_(d, i, 4, (uint64_t)s & 0xFFFFFFFFull); } return;
    case II_MADDUBSW: for (int i = 0; i < w / 2; i++) { int64_t s = (int64_t)(uint8_t)a[2 * i] * (int8_t)b[2 * i] + (int64_t)(uint8_t)a[2 * i + 1] * (int8_t)b[2 * i + 1]; se_(d, i, 2, (uint64_t)sat_s(s, 16) & 0xFFFF); } return;
    case II_SADBW: for (int i = 0; i < w / 8; i++) { uint32_t s = 0; for (int j = 0; j < 8; j++) { int x = a[8 * i + j], y = b[8 * i + j]; s += (uint32_t)(x > y ? x - y : y - x); } se_(d, i, 8, s); } return;
    }
    for (int i = 0; i < n; i++) {
        uint64_t x = el_(a, i, es), y = el_(b, i, es), r2 = 0; int64_t sx = sx_(x, bits), sy = sx_(y, bits);
        switch (kind) {
        case II_ADD: r2 = x + y; break; case II_SUB: r2 = x - y; break;
        case II_ADDS: r2 = (uint64_t)sat_s(sx + sy, bits); break; case II_SUBS: r2 = (uint64_t)sat_s(sx - sy, bits); break;
        case II_ADDUS: r2 = (uint64_t)sat_u((int64_t)(x + y), bits); break; case II_SUBUS: r2 = x > y ? x - y : 0; break;
        case II_MINS: r2 = (uint64_t)(sx < sy ? sx : sy); break; case II_MAXS: r2 = (uint64_t)(sx > sy ? sx : sy); break;
        case II_MINU: r2 = x < y ? x : y; break; case II_MAXU: r2 = x > y ? x : y; break;
        case II_AVG: r2 = (x + y + 1) >> 1; break;
        case II_CMPEQ: r2 = x == y ? M : 0; break; case II_CMPGT: r2 = sx > sy ? M : 0; break;
        case II_MULLO: r2 = x * y; break;
        case II_MULHS: r2 = (uint64_t)((sx * sy) >> bits); break; case II_MULHU: r2 = (x * y) >> bits; break;
        case II_MULHRSW: r2 = (uint64_t)((((sx * sy) >> 14) + 1) >> 1); break;
        case II_SIGN: r2 = sy < 0 ? (uint64_t)(-sx) : sy == 0 ? 0 : x; break;
        }
        if (MODEL_NEG && kind == II_MAXU && es == 2) r2 = sx > sy ? x : y;       // negative control: unsigned max done as signed
        se_(d, i, es, r2 & M);
    }
}
#endif
