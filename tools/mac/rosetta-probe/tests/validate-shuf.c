// Checks isa-model-shuf.h against independent arm64 NEON formulations where one exists (zip1/zip2 for unpack, sqxtn/sqxtun/uqxtn for the packs, tbl for pshufb, addp/sub of
// uzp1/uzp2 for the integer horizontal ops, ext for palignr, bsl for the blends) and against explicit element formulas for the permutes:  clang -O1 -I.. validate-shuf.c -o v && ./v
#include <arm_neon.h>
#include "../isa-model-shuf.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static long bad, total;
static void chk(const char *w, const uint8_t *got, const uint8_t *want, int n, int imm) { total++; if (memcmp(got, want, (size_t)n)) { if (bad++ < 12) { printf("%s imm=%d\n got ", w, imm); for (int i = n - 1; i >= 0; i--) printf("%02x", got[i]); printf("\nwant ", imm); for (int i = n - 1; i >= 0; i--) printf("%02x", want[i]); printf("\n"); } } }
static void fill(uint8_t *p, int n) { for (int i = 0; i < n; i++) p[i] = (uint8_t)rnd(); if (rnd() % 3 == 0) for (int i = 0; i < n; i++) switch (rnd() % 6) { case 0: p[i] = 0; break; case 1: p[i] = 0xFF; break; case 2: p[i] = 0x80; break; case 3: p[i] = 0x7F; break; default: break; } }
static void run(int kind, int es, int opt, int w, int imm, const IRec *r, Out *o) { Op op; memset(&op, 0, sizeof op); op.p = (void *)(intptr_t)(kind | (es << 8) | (opt << 16)); op.alias = AL_DD; Ctx c = {&op, w, imm, 0x1F80}; memset(o, 0, sizeof *o); m_shuf(&c, r, o); }
int main(void) {
    for (long it = 0; it < 3000000; it++) {
        IRec r; for (int i = 0; i < 128; i++) ((uint8_t *)&r)[i] = 0; fill(r.a, 32); fill(r.b, 32); fill(r.c, 32); Out o, ref; int imm = (int)(rnd() & 255), w = (rnd() & 1) ? 32 : 16;
        for (int L = 0; L < w / 16; L++) {
            uint8x16_t a = vld1q_u8(r.a + 16 * L), b = vld1q_u8(r.b + 16 * L);
            // unpack low/high for all element sizes (zip1/zip2 interleave a (even) and b (odd) elements; low = zip1, high = zip2)
            for (int hi = 0; hi < 2; hi++) { int sizes[4] = {1, 2, 4, 8}; for (int q = 0; q < 4; q++) { int es = sizes[q]; run(S_UNPCK, es, hi, w, 0, &r, &o); uint8x16_t z;
                if (es == 1) z = hi ? vzip2q_u8(a, b) : vzip1q_u8(a, b); else if (es == 2) z = vreinterpretq_u8_u16(hi ? vzip2q_u16(vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b)) : vzip1q_u16(vreinterpretq_u16_u8(a), vreinterpretq_u16_u8(b)));
                else if (es == 4) z = vreinterpretq_u8_u32(hi ? vzip2q_u32(vreinterpretq_u32_u8(a), vreinterpretq_u32_u8(b)) : vzip1q_u32(vreinterpretq_u32_u8(a), vreinterpretq_u32_u8(b)));
                else z = vreinterpretq_u8_u64(hi ? vzip2q_u64(vreinterpretq_u64_u8(a), vreinterpretq_u64_u8(b)) : vzip1q_u64(vreinterpretq_u64_u8(a), vreinterpretq_u64_u8(b)));
                uint8_t t[16]; vst1q_u8(t, z); chk("unpck", o.r + 16 * L, t, 16, es * 10 + hi); } }
            // packs: low half from a, high half from b
            { run(S_PACK, 2, 0, w, 0, &r, &o); int16x8_t pk = vcombine_s16(vqmovn_s32(vreinterpretq_s32_u8(a)), vqmovn_s32(vreinterpretq_s32_u8(b))); chk("packssdw", o.r + 16 * L, (uint8_t *)&pk, 16, 0);
              run(S_PACK, 1, 1, w, 0, &r, &o); int8x16_t pb = vcombine_s8(vqmovn_s16(vreinterpretq_s16_u8(a)), vqmovn_s16(vreinterpretq_s16_u8(b))); chk("packsswb", o.r + 16 * L, (uint8_t *)&pb, 16, 0);
              run(S_PACK, 1, 2, w, 0, &r, &o); uint8x16_t pu = vcombine_u8(vqmovun_s16(vreinterpretq_s16_u8(a)), vqmovun_s16(vreinterpretq_s16_u8(b))); chk("packuswb", o.r + 16 * L, (uint8_t *)&pu, 16, 0);
              run(S_PACK, 2, 3, w, 0, &r, &o); uint16x8_t pw = vcombine_u16(vqmovun_s32(vreinterpretq_s32_u8(a)), vqmovun_s32(vreinterpretq_s32_u8(b))); chk("packusdw", o.r + 16 * L, (uint8_t *)&pw, 16, 0); }
            // pshufb through tbl: index with bit 7 set -> out of range (tbl gives 0)
            { run(S_PSHUFB, 1, 0, w, 0, &r, &o); uint8x16_t idx = b; uint8x16_t m = vcltzq_s8(vreinterpretq_s8_u8(b)); idx = vorrq_u8(vandq_u8(b, vdupq_n_u8(15)), vandq_u8(m, vdupq_n_u8(0x80))); uint8x16_t t = vqtbl1q_u8(a, idx); chk("pshufb", o.r + 16 * L, (uint8_t *)&t, 16, 0); }
            // integer horizontal: pairs of a then pairs of b
            { run(S_PHADD, 2, 0, w, 0, &r, &o); int16x8_t sa = vreinterpretq_s16_u8(a), sb = vreinterpretq_s16_u8(b); int16x8_t h = vcombine_s16(vpadd_s16(vget_low_s16(sa), vget_high_s16(sa)), vpadd_s16(vget_low_s16(sb), vget_high_s16(sb))); chk("phaddw", o.r + 16 * L, (uint8_t *)&h, 16, 0);
              run(S_PHADD, 2, 2, w, 0, &r, &o); int16x8_t ev = vuzp1q_s16(sa, sb), od = vuzp2q_s16(sa, sb); int16x8_t hs = vqaddq_s16(ev, od); chk("phaddsw", o.r + 16 * L, (uint8_t *)&hs, 16, 0);
              run(S_PHADD, 2, 1, w, 0, &r, &o); int16x8_t hd = vsubq_s16(ev, od); chk("phsubw", o.r + 16 * L, (uint8_t *)&hd, 16, 0);
              run(S_PHADD, 2, 3, w, 0, &r, &o); int16x8_t hds = vqsubq_s16(ev, od); chk("phsubsw", o.r + 16 * L, (uint8_t *)&hds, 16, 0);
              run(S_PHADD, 4, 0, w, 0, &r, &o); int32x4_t da = vreinterpretq_s32_u8(a), db = vreinterpretq_s32_u8(b); int32x4_t h4 = vaddq_s32(vuzp1q_s32(da, db), vuzp2q_s32(da, db)); chk("phaddd", o.r + 16 * L, (uint8_t *)&h4, 16, 0);
              run(S_PHADD, 4, 1, w, 0, &r, &o); int32x4_t h5 = vsubq_s32(vuzp1q_s32(da, db), vuzp2q_s32(da, db)); chk("phsubd", o.r + 16 * L, (uint8_t *)&h5, 16, 0); }
            // palignr: bytes (b : a) -> concat(a high, b low) shifted right by imm bytes
            { run(S_PALIGNR, 1, 0, w, imm, &r, &o); uint8_t t[16]; if (imm < 16) { uint8_t cat[32]; memcpy(cat, r.b + 16 * L, 16); memcpy(cat + 16, r.a + 16 * L, 16); memcpy(t, cat + imm, 16); } else if (imm < 32) { memset(t, 0, 16); memcpy(t, r.a + 16 * L + (imm - 16), 32 - imm); } else memset(t, 0, 16); chk("palignr", o.r + 16 * L, t, 16, imm); }
            // blendv: top bit of the mask element selects b
            { uint8x16_t cc = vld1q_u8(r.c + 16 * L); uint8x16_t m8 = vreinterpretq_u8_s8(vshrq_n_s8(vreinterpretq_s8_u8(cc), 7)); run(S_BLENDV, 1, 0, w, 0, &r, &o); uint8x16_t t = vbslq_u8(m8, b, a); chk("pblendvb", o.r + 16 * L, (uint8_t *)&t, 16, 0);
              uint32x4_t m32 = vreinterpretq_u32_s32(vshrq_n_s32(vreinterpretq_s32_u8(cc), 31)); run(S_BLENDV, 4, 0, w, 0, &r, &o); uint32x4_t t32 = vbslq_u32(m32, vreinterpretq_u32_u8(b), vreinterpretq_u32_u8(a)); chk("blendvps", o.r + 16 * L, (uint8_t *)&t32, 16, 0);
              uint64x2_t m64 = vreinterpretq_u64_s64(vshrq_n_s64(vreinterpretq_s64_u8(cc), 63)); run(S_BLENDV, 8, 0, w, 0, &r, &o); uint64x2_t t64 = vbslq_u64(m64, vreinterpretq_u64_u8(b), vreinterpretq_u64_u8(a)); chk("blendvpd", o.r + 16 * L, (uint8_t *)&t64, 16, 0); }
        }
        // permutes by explicit element formulas
        { run(S_PERMQ, 8, 0, 32, imm, &r, &o); uint64_t q[4]; memcpy(q, r.b, 32); uint64_t t[4]; for (int i = 0; i < 4; i++) t[i] = q[(imm >> (2 * i)) & 3]; chk("permq", o.r, (uint8_t *)t, 32, imm); }
        { run(S_PERMD, 4, 0, 32, 0, &r, &o); uint32_t ix[8], dt[8], t[8]; memcpy(ix, r.a, 32); memcpy(dt, r.b, 32); for (int i = 0; i < 8; i++) t[i] = dt[ix[i] & 7]; chk("permd", o.r, (uint8_t *)t, 32, 0); }
        { run(S_PERM2X128, 16, 0, 32, imm, &r, &o); uint8_t t[32]; memset(t, 0, 32); for (int h = 0; h < 2; h++) { int sel = (imm >> (4 * h)) & 15; if (sel & 8) continue; const uint8_t *src = (sel & 2) ? r.b : r.a; memcpy(t + 16 * h, src + 16 * (sel & 1), 16); } chk("perm2x128", o.r, t, 32, imm); }
        { run(S_SHUFPS, 4, 0, w, imm, &r, &o); uint32_t A4[8], B4[8], t[8]; memcpy(A4, r.a, 32); memcpy(B4, r.b, 32); for (int L = 0; L < w / 16; L++) { t[4 * L] = A4[4 * L + (imm & 3)]; t[4 * L + 1] = A4[4 * L + ((imm >> 2) & 3)]; t[4 * L + 2] = B4[4 * L + ((imm >> 4) & 3)]; t[4 * L + 3] = B4[4 * L + ((imm >> 6) & 3)]; } chk("shufps", o.r, (uint8_t *)t, w, imm); }
        { run(S_BLENDI, 4, 0, w, imm, &r, &o); uint32_t A4[8], B4[8], t[8]; memcpy(A4, r.a, 32); memcpy(B4, r.b, 32); for (int i = 0; i < w / 4; i++) t[i] = ((imm >> i) & 1) ? B4[i] : A4[i]; chk("blendps", o.r, (uint8_t *)t, w, imm); }
        { run(S_INSERTPS, 4, 0, 16, imm, &r, &o); uint32_t A4[4], B4[4], t[4]; memcpy(A4, r.a, 16); memcpy(B4, r.b, 16); memcpy(t, A4, 16); t[(imm >> 4) & 3] = B4[(imm >> 6) & 3]; for (int i = 0; i < 4; i++) if ((imm >> i) & 1) t[i] = 0; chk("insertps", o.r, (uint8_t *)t, 16, imm); }
    }
    printf("validate-shuf: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
