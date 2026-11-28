// Checks the software models of isa-model-move.h against arm64 hardware (NEON intrinsics / plain loads and stores): native build on arm64,
//   clang -O1 -I.. validate-move.c -o v && ./v
#include <arm_neon.h>
#include "../isa-model-move.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static void fill(IRec *r) { for (int i = 0; i < 128; i++) ((uint8_t *)r)[i] = (uint8_t)rnd(); if (rnd() % 4 == 0) for (int i = 0; i < 128; i++) ((uint8_t *)r)[i] &= ((uint8_t *)r)[(i * 7) & 127]; }
static long bad, total;
static void check(const char *what, const uint8_t *got, const uint8_t *want, int n) { total++; if (memcmp(got, want, n)) { if (bad++ < 10) { printf("%s: mismatch\n", what); } } }
int main(void) {
    for (long it = 0; it < 4000000; it++) {
        IRec r; fill(&r); Out o, e; Op op; Ctx cx;
        memset(&op, 0, sizeof op); op.cr = 32;
        for (int w = 16; w <= 32; w += 16) {
            op.w = w; cx.op = &op; cx.w = w; cx.imm = 0; cx.mx = 0;
            // copy (distinct and dst==src)
            for (int al = 0; al < 2; al++) { op.alias = al ? AL_DA : AL_DD; memset(&o, 0xAA, sizeof o); m_copy(&cx, &r, &o);
                const uint8_t *src = al ? r.a : r.b; memset(&e, 0, sizeof e); uint8x16_t lo = vld1q_u8(src), hi = w == 32 ? vld1q_u8(src + 16) : vdupq_n_u8(0); vst1q_u8(e.r, lo); vst1q_u8(e.r + 16, hi); check("copy", o.r, e.r, 32); }
            // load at an offset
            op.alias = AL_MEM; op.memoff = (int)(rnd() % 32); op.sz = w; memset(&o, 0xAA, sizeof o); m_load(&cx, &r, &o);
            { const uint8_t *src = (const uint8_t *)&r + 64 + op.memoff; memset(&e, 0, sizeof e); vst1q_u8(e.r, vld1q_u8(src)); if (w == 32) vst1q_u8(e.r + 16, vld1q_u8(src + 16)); check("load", o.r, e.r, 32); }
            // store: image = d||c, then the vector register (a) stored at the offset
            op.memoff = (int)(rnd() % (65 - w)); op.p2 = 0; memcpy(o.r, r.d, 32); memcpy(o.x, r.c, 32); m_store(&cx, &r, &o);
            { uint8_t img[64]; vst1q_u8(img, vld1q_u8(r.d)); vst1q_u8(img + 16, vld1q_u8(r.d + 16)); vst1q_u8(img + 32, vld1q_u8(r.c)); vst1q_u8(img + 48, vld1q_u8(r.c + 16));
              vst1q_u8(img + op.memoff, vld1q_u8(r.a)); if (w == 32) vst1q_u8(img + op.memoff + 16, vld1q_u8(r.a + 16)); check("store", (uint8_t *)&o, img, 64); }
            // broadcasts of 1/2/4/8 bytes from src2 and from memory
            for (int sz = 1; sz <= 8; sz *= 2) {
                op.sz = sz; op.alias = AL_DD; memset(&o, 0xAA, sizeof o); m_bcast(&cx, &r, &o); uint8x16_t v;
                switch (sz) { case 1: v = vdupq_n_u8(r.b[0]); break; case 2: v = vreinterpretq_u8_u16(vdupq_n_u16(*(uint16_t *)r.b)); break; case 4: v = vreinterpretq_u8_u32(vdupq_n_u32(*(uint32_t *)r.b)); break; default: v = vreinterpretq_u8_u64(vdupq_n_u64(*(uint64_t *)r.b)); }
                memset(&e, 0, sizeof e); vst1q_u8(e.r, v); if (w == 32) vst1q_u8(e.r + 16, v); check("bcast", o.r, e.r, 32);
            }
            op.sz = 16; op.alias = AL_MEM; op.memoff = (int)(rnd() % 16); if (w == 32) { memset(&o, 0xAA, sizeof o); m_bcast(&cx, &r, &o); uint8x16_t v = vld1q_u8((const uint8_t *)&r + 64 + op.memoff); memset(&e, 0, sizeof e); vst1q_u8(e.r, v); vst1q_u8(e.r + 16, v); check("bcast128", o.r, e.r, 32); }
            // movmskps / movmskpd / pmovmskb
            memset(&o, 0, sizeof o); m_movmskps(&cx, &r, &o); { uint32_t m = 0; for (int h = 0; h < w / 16; h++) { uint32x4_t sg = vshrq_n_u32(vld1q_u32((const uint32_t *)(r.b + 16 * h)), 31); static const uint32_t wt[4] = {1, 2, 4, 8}; m |= vaddvq_u32(vmulq_u32(sg, vld1q_u32(wt))) << (4 * h); } check("movmskps", (uint8_t *)&o.x[0], (uint8_t *)&(uint64_t){m}, 8); }
            memset(&o, 0, sizeof o); m_movmskpd(&cx, &r, &o); { uint32_t m = 0; for (int h = 0; h < w / 16; h++) { uint64x2_t sg = vshrq_n_u64(vld1q_u64((const uint64_t *)(r.b + 16 * h)), 63); m |= (uint32_t)(vgetq_lane_u64(sg, 0) | (vgetq_lane_u64(sg, 1) << 1)) << (2 * h); } check("movmskpd", (uint8_t *)&o.x[0], (uint8_t *)&(uint64_t){m}, 8); }
            memset(&o, 0, sizeof o); m_pmovmskb(&cx, &r, &o); { uint32_t m = 0; static const uint8_t wt[16] = {1, 2, 4, 8, 16, 32, 64, 128, 1, 2, 4, 8, 16, 32, 64, 128}; for (int h = 0; h < w / 16; h++) { uint8x16_t sg = vshrq_n_u8(vld1q_u8(r.b + 16 * h), 7), t = vmulq_u8(sg, vld1q_u8(wt)); m |= ((uint32_t)vaddv_u8(vget_low_u8(t)) | ((uint32_t)vaddv_u8(vget_high_u8(t)) << 8)) << (16 * h); } check("pmovmskb", (uint8_t *)&o.x[0], (uint8_t *)&(uint64_t){m}, 8); }
            // movshdup / movsldup / movddup
            for (int kind = 1; kind <= 3; kind++) { op.p = (void *)(intptr_t)kind; op.alias = AL_DD; memset(&o, 0xAA, sizeof o); m_dup_r(&cx, &r, &o); memset(&e, 0, sizeof e);
                for (int h = 0; h < w / 16; h++) { uint32x4_t v = vld1q_u32((const uint32_t *)(r.b + 16 * h)), t; uint64x2_t q = vreinterpretq_u64_u32(v);
                    if (kind == 3) t = vtrn2q_u32(v, v); else if (kind == 2) t = vtrn1q_u32(v, v); else t = vreinterpretq_u32_u64(vtrn1q_u64(q, q)); vst1q_u32((uint32_t *)(e.r + 16 * h), t); }
                check("dup", o.r, e.r, 32); }
        }
        // 128-bit merges
        op.w = 16; op.sz = 4; for (int k = 0; k < 2; k++) { op.sz = k ? 8 : 4; cx.op = &op; cx.w = 16; memset(&o, 0xAA, sizeof o); m_movss(&cx, &r, &o);
            uint8x16_t a = vld1q_u8(r.a), b = vld1q_u8(r.b); uint8x16_t v = k ? vreinterpretq_u8_u64(vsetq_lane_u64(vgetq_lane_u64(vreinterpretq_u64_u8(b), 0), vreinterpretq_u64_u8(a), 0)) : vreinterpretq_u8_u32(vsetq_lane_u32(vgetq_lane_u32(vreinterpretq_u32_u8(b), 0), vreinterpretq_u32_u8(a), 0));
            memset(&e, 0, sizeof e); vst1q_u8(e.r, v); check("movss", o.r, e.r, 32); }
        { cx.op = &op; cx.w = 16; uint64x2_t a = vld1q_u64((const uint64_t *)r.a), b = vld1q_u64((const uint64_t *)r.b);
          memset(&o, 0xAA, sizeof o); m_lhps(&cx, &r, &o); memset(&e, 0, sizeof e); vst1q_u64((uint64_t *)e.r, vcombine_u64(vget_low_u64(a), vget_low_u64(b))); check("movlhps", o.r, e.r, 32);
          memset(&o, 0xAA, sizeof o); m_hlps(&cx, &r, &o); memset(&e, 0, sizeof e); vst1q_u64((uint64_t *)e.r, vcombine_u64(vget_high_u64(b), vget_high_u64(a))); check("movhlps", o.r, e.r, 32);
          op.alias = AL_MEM; op.memoff = (int)(rnd() % 16); const uint64_t *m = (const uint64_t *)((const uint8_t *)&r + 64 + op.memoff); uint64_t mv; memcpy(&mv, m, 8);
          memset(&o, 0xAA, sizeof o); m_lps_load(&cx, &r, &o); memset(&e, 0, sizeof e); vst1q_u64((uint64_t *)e.r, vcombine_u64(vcreate_u64(mv), vget_high_u64(a))); check("movlps", o.r, e.r, 32);
          memset(&o, 0xAA, sizeof o); m_hps_load(&cx, &r, &o); memset(&e, 0, sizeof e); vst1q_u64((uint64_t *)e.r, vcombine_u64(vget_low_u64(a), vcreate_u64(mv))); check("movhps", o.r, e.r, 32);
          op.sz = 4; memset(&o, 0xAA, sizeof o); op.alias = AL_DD; m_movd_to_x(&cx, &r, &o); memset(&e, 0, sizeof e); vst1q_u32((uint32_t *)e.r, vsetq_lane_u32(*(const uint32_t *)r.b, vdupq_n_u32(0), 0)); check("movd", o.r, e.r, 32);
          op.sz = 8; memset(&o, 0xAA, sizeof o); m_movd_to_x(&cx, &r, &o); memset(&e, 0, sizeof e); vst1q_u64((uint64_t *)e.r, vsetq_lane_u64(*(const uint64_t *)r.b, vdupq_n_u64(0), 0)); check("movq", o.r, e.r, 32);
          memset(&o, 0, sizeof o); op.sz = 4; m_movd_from_x(&cx, &r, &o); uint64_t w4 = vgetq_lane_u32(vld1q_u32((const uint32_t *)r.b), 0); check("movd_from", (uint8_t *)&o.x[0], (uint8_t *)&w4, 8);
          memset(&o, 0, sizeof o); op.sz = 8; m_movd_from_x(&cx, &r, &o); uint64_t w8 = vgetq_lane_u64(vld1q_u64((const uint64_t *)r.b), 0); check("movq_from", (uint8_t *)&o.x[0], (uint8_t *)&w8, 8); }
    }
    printf("validate-move: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
