// Checks isa-model-iarith.h against arm64 NEON (add/sub, saturating add/sub, min/max, rounding average, compares, multiplies, pairwise multiply-add, sad, abs, extends, shifts
// with the x86 count rules built around ushl/sshl) and scalar __int128 formulations:  clang -O1 -I.. validate-iarith.c -o v && ./v
#include <arm_neon.h>
#include "../isa-model-iarith.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static long bad, total;
static void chk(const char *w, const uint8_t *got, const uint8_t *want, int n, int tag) { total++; if (memcmp(got, want, (size_t)n)) { if (bad++ < 12) { printf("%s tag=%d\n got ", w, tag); for (int i = n - 1; i >= 0; i--) printf("%02x", got[i]); printf("\nwant "); for (int i = n - 1; i >= 0; i--) printf("%02x", want[i]); printf("\n"); } } }
static void fill(uint8_t *p, int n) { for (int i = 0; i < n; i++) p[i] = (uint8_t)rnd(); if (rnd() % 3 == 0) for (int i = 0; i < n; i++) switch (rnd() % 6) { case 0: p[i] = 0; break; case 1: p[i] = 0xFF; break; case 2: p[i] = 0x80; break; case 3: p[i] = 0x7F; break; default: break; } }
static void run(int kind, int es, int opt, int es2, int imm, const IRec *r, Out *o) { Op op; memset(&op, 0, sizeof op); op.p = (void *)(intptr_t)(kind | (es << 8) | (opt << 16) | (es2 << 24)); op.alias = AL_DD; Ctx c = {&op, 16, imm, 0x1F80}; memset(o, 0, sizeof *o); m_ia(&c, r, o); }
#define V(T, P) vreinterpretq_##T##_u8(P)
int main(void) {
    for (long it = 0; it < 4000000; it++) {
        IRec r; memset(&r, 0, sizeof r); fill(r.a, 16); fill(r.b, 16); Out o; uint8x16_t a = vld1q_u8(r.a), b = vld1q_u8(r.b); int imm = (int)(rnd() & 255);
#define T(NAME, KIND, ES, OPT, EXPR) { run(KIND, ES, OPT, 0, 0, &r, &o); uint8x16_t t = EXPR; chk(NAME, o.r, (uint8_t *)&t, 16, 0); }
        T("add8", II_ADD, 1, 0, vaddq_u8(a, b)) T("add16", II_ADD, 2, 0, vreinterpretq_u8_u16(vaddq_u16(V(u16, a), V(u16, b)))) T("add32", II_ADD, 4, 0, vreinterpretq_u8_u32(vaddq_u32(V(u32, a), V(u32, b)))) T("add64", II_ADD, 8, 0, vreinterpretq_u8_u64(vaddq_u64(V(u64, a), V(u64, b))))
        T("sub8", II_SUB, 1, 0, vsubq_u8(a, b)) T("sub64", II_SUB, 8, 0, vreinterpretq_u8_u64(vsubq_u64(V(u64, a), V(u64, b))))
        T("adds8", II_ADDS, 1, 0, vreinterpretq_u8_s8(vqaddq_s8(V(s8, a), V(s8, b)))) T("adds16", II_ADDS, 2, 0, vreinterpretq_u8_s16(vqaddq_s16(V(s16, a), V(s16, b))))
        T("addus8", II_ADDUS, 1, 0, vqaddq_u8(a, b)) T("addus16", II_ADDUS, 2, 0, vreinterpretq_u8_u16(vqaddq_u16(V(u16, a), V(u16, b))))
        T("subs8", II_SUBS, 1, 0, vreinterpretq_u8_s8(vqsubq_s8(V(s8, a), V(s8, b)))) T("subs16", II_SUBS, 2, 0, vreinterpretq_u8_s16(vqsubq_s16(V(s16, a), V(s16, b))))
        T("subus8", II_SUBUS, 1, 0, vqsubq_u8(a, b)) T("subus16", II_SUBUS, 2, 0, vreinterpretq_u8_u16(vqsubq_u16(V(u16, a), V(u16, b))))
        T("mins8", II_MINS, 1, 0, vreinterpretq_u8_s8(vminq_s8(V(s8, a), V(s8, b)))) T("mins32", II_MINS, 4, 0, vreinterpretq_u8_s32(vminq_s32(V(s32, a), V(s32, b)))) T("minu16", II_MINU, 2, 0, vreinterpretq_u8_u16(vminq_u16(V(u16, a), V(u16, b)))) T("minu32", II_MINU, 4, 0, vreinterpretq_u8_u32(vminq_u32(V(u32, a), V(u32, b))))
        T("maxs16", II_MAXS, 2, 0, vreinterpretq_u8_s16(vmaxq_s16(V(s16, a), V(s16, b)))) T("maxu8", II_MAXU, 1, 0, vmaxq_u8(a, b)) T("maxu32", II_MAXU, 4, 0, vreinterpretq_u8_u32(vmaxq_u32(V(u32, a), V(u32, b))))
        T("avg8", II_AVG, 1, 0, vrhaddq_u8(a, b)) T("avg16", II_AVG, 2, 0, vreinterpretq_u8_u16(vrhaddq_u16(V(u16, a), V(u16, b))))
        T("cmpeq8", II_CMPEQ, 1, 0, vceqq_u8(a, b)) T("cmpeq64", II_CMPEQ, 8, 0, vreinterpretq_u8_u64(vceqq_u64(V(u64, a), V(u64, b)))) T("cmpgt16", II_CMPGT, 2, 0, vreinterpretq_u8_u16(vcgtq_s16(V(s16, a), V(s16, b)))) T("cmpgt64", II_CMPGT, 8, 0, vreinterpretq_u8_u64(vcgtq_s64(V(s64, a), V(s64, b))))
        T("mullw", II_MULLO, 2, 0, vreinterpretq_u8_u16(vmulq_u16(V(u16, a), V(u16, b)))) T("mulld", II_MULLO, 4, 0, vreinterpretq_u8_u32(vmulq_u32(V(u32, a), V(u32, b))))
        { run(II_MULHS, 2, 0, 0, 0, &r, &o); int32x4_t lo = vmull_s16(vget_low_s16(V(s16, a)), vget_low_s16(V(s16, b))), hi = vmull_high_s16(V(s16, a), V(s16, b)); int16x8_t t = vcombine_s16(vshrn_n_s32(lo, 16), vshrn_n_s32(hi, 16)); chk("mulhw", o.r, (uint8_t *)&t, 16, 0); }
        { run(II_MULHU, 2, 0, 0, 0, &r, &o); uint32x4_t lo = vmull_u16(vget_low_u16(V(u16, a)), vget_low_u16(V(u16, b))), hi = vmull_high_u16(V(u16, a), V(u16, b)); uint16x8_t t = vcombine_u16(vshrn_n_u32(lo, 16), vshrn_n_u32(hi, 16)); chk("mulhuw", o.r, (uint8_t *)&t, 16, 0); }
        { run(II_MULHRSW, 2, 0, 0, 0, &r, &o); uint8_t t[16]; for (int i = 0; i < 8; i++) { int32_t p = (int32_t)(int16_t)el_(r.a, i, 2) * (int16_t)el_(r.b, i, 2); st_(t + 2 * i, 2, (uint64_t)(uint16_t)(((p >> 14) + 1) >> 1)); } chk("mulhrsw", o.r, t, 16, 0); }
        { run(II_MULDQ, 8, 0, 0, 0, &r, &o); int64x2_t t = vmull_s32(vmovn_s64(V(s64, a)), vmovn_s64(V(s64, b))); chk("muldq", o.r, (uint8_t *)&t, 16, 0); run(II_MULUDQ, 8, 0, 0, 0, &r, &o); uint64x2_t u = vmull_u32(vmovn_u64(V(u64, a)), vmovn_u64(V(u64, b))); chk("muludq", o.r, (uint8_t *)&u, 16, 0); }
        { run(II_MADDWD, 4, 0, 0, 0, &r, &o); int32x4_t lo = vmull_s16(vget_low_s16(V(s16, a)), vget_low_s16(V(s16, b))), hi = vmull_high_s16(V(s16, a), V(s16, b)); int32x4_t t = vpaddq_s32(lo, hi); chk("maddwd", o.r, (uint8_t *)&t, 16, 0); }
        { run(II_MADDUBSW, 2, 0, 0, 0, &r, &o); int16x8_t pe = vmulq_s16(vreinterpretq_s16_u16(vmovl_u8(vget_low_u8(a))), vmovl_s8(vget_low_s8(V(s8, b)))); (void)pe;
          uint8_t t[16]; for (int i = 0; i < 8; i++) { int32_t sm = (int32_t)r.a[2 * i] * (int8_t)r.b[2 * i] + (int32_t)r.a[2 * i + 1] * (int8_t)r.b[2 * i + 1]; sm = sm > 32767 ? 32767 : sm < -32768 ? -32768 : sm; st_(t + 2 * i, 2, (uint64_t)(uint16_t)sm); } chk("maddubsw", o.r, t, 16, 0); }
        { run(II_SADBW, 8, 0, 0, 0, &r, &o); uint8x16_t ad = vabdq_u8(a, b); uint64x2_t t = vpaddlq_u32(vpaddlq_u16(vpaddlq_u8(ad))); chk("sadbw", o.r, (uint8_t *)&t, 16, 0); }
        { run(II_SIGN, 1, 0, 0, 0, &r, &o); int8x16_t sb = V(s8, b), sa = V(s8, a); int8x16_t neg = vnegq_s8(sa); uint8x16_t isneg = vcltzq_s8(sb), iszero = vceqzq_s8(sb); int8x16_t t = vbslq_s8(isneg, neg, sa); t = vbicq_s8(t, V(s8, iszero)); chk("signb", o.r, (uint8_t *)&t, 16, 0); }
        { run(II_ABS, 4, 0, 0, 0, &r, &o); int32x4_t t = vabsq_s32(V(s32, b)); chk("absd", o.r, (uint8_t *)&t, 16, 0); run(II_ABS, 1, 0, 0, 0, &r, &o); int8x16_t t1 = vabsq_s8(V(s8, b)); chk("absb", o.r, (uint8_t *)&t1, 16, 0); }
        { run(II_MOVX, 2, 1, 1, 0, &r, &o); int16x8_t t = vmovl_s8(vget_low_s8(V(s8, b))); chk("pmovsxbw", o.r, (uint8_t *)&t, 16, 0); run(II_MOVX, 4, 0, 2, 0, &r, &o); uint32x4_t u = vmovl_u16(vget_low_u16(V(u16, b))); chk("pmovzxwd", o.r, (uint8_t *)&u, 16, 0);
          run(II_MOVX, 8, 1, 4, 0, &r, &o); int64x2_t q = vmovl_s32(vget_low_s32(V(s32, b))); chk("pmovsxdq", o.r, (uint8_t *)&q, 16, 0); }
        // shifts: x86 count rule: counts >= element width give 0 (logical) or the sign (arithmetic); arm64 sshl/ushl take the low byte of the count as a signed value, so the reference clamps first
        for (int es = 2; es <= 8; es *= 2) { for (int dir = 0; dir < 3; dir++) {
            run(II_SHI, es, dir, 0, imm, &r, &o); uint8_t t[16]; for (int i = 0; i < 16 / es; i++) { uint64_t v = el_(r.b, i, es); int bt = es * 8; uint64_t m = mskb(bt), res; if (dir == 0) res = imm >= bt ? 0 : (v << imm) & m; else if (dir == 1) res = imm >= bt ? 0 : v >> imm; else { int64_t sv = sx_(v, bt); res = (uint64_t)(sv >> (imm >= bt ? bt - 1 : imm)) & m; } st_(t + i * es, es, res); } chk("shift imm", o.r, t, 16, es * 10 + dir); } }
        { run(II_SHI, 4, 0, 0, imm & 31, &r, &o); int32x4_t t = vshlq_s32(V(s32, b), vdupq_n_s32(imm & 31)); chk("pslld hw", o.r, (uint8_t *)&t, 16, imm); run(II_SHI, 4, 1, 0, imm & 31, &r, &o); uint32x4_t u = vshlq_u32(V(u32, b), vdupq_n_s32(-(imm & 31))); chk("psrld hw", o.r, (uint8_t *)&u, 16, imm); run(II_SHI, 8, 2, 0, imm & 63, &r, &o); int64x2_t q = vshlq_s64(V(s64, b), vdupq_n_s64(-(imm & 63))); chk("psraq hw", o.r, (uint8_t *)&q, 16, imm); }
        { run(II_SHDQ, 1, SH_L, 0, imm, &r, &o); uint8_t t[16]; memset(t, 0, 16); if (imm < 16) memcpy(t + imm, r.b, 16 - imm); chk("pslldq", o.r, t, 16, imm); run(II_SHDQ, 1, SH_R, 0, imm, &r, &o); memset(t, 0, 16); if (imm < 16) memcpy(t, r.b + imm, 16 - imm); chk("psrldq", o.r, t, 16, imm); }
        // per-element variable shifts: 32-bit counts with the count clamp, and 64-bit counts using the whole 64-bit value
        { run(II_SHV, 4, SH_L, 0, 0, &r, &o); uint8_t t[16]; for (int i = 0; i < 4; i++) { uint32_t v = (uint32_t)el_(r.a, i, 4), cnt = (uint32_t)el_(r.b, i, 4); st_(t + 4 * i, 4, cnt > 31 ? 0 : (uint32_t)(v << cnt)); } chk("psllvd", o.r, t, 16, 0);
          run(II_SHV, 8, SH_R, 0, 0, &r, &o); for (int i = 0; i < 2; i++) { uint64_t v = el_(r.a, i, 8), cnt = el_(r.b, i, 8); st_(t + 8 * i, 8, cnt > 63 ? 0 : v >> cnt); } chk("psrlvq", o.r, t, 16, 0);
          run(II_SHV, 4, SH_A, 0, 0, &r, &o); for (int i = 0; i < 4; i++) { int32_t v = (int32_t)el_(r.a, i, 4); uint32_t cnt = (uint32_t)el_(r.b, i, 4); st_(t + 4 * i, 4, (uint32_t)(v >> (cnt > 31 ? 31 : cnt))); } chk("psravd", o.r, t, 16, 0); }
        { run(II_HMINPOS, 2, 0, 0, 0, &r, &o); uint16x8_t w = V(u16, b); uint16_t mn = vminvq_u16(w); int ix = 0; for (int i = 0; i < 8; i++) if (el_(r.b, i, 2) == mn) { ix = i; break; } uint8_t t[16]; memset(t, 0, 16); st_(t, 2, mn); st_(t + 2, 2, (uint64_t)ix); chk("phminposuw", o.r, t, 16, 0); }
        { run(II_PTEST, 1, 0, 0, 0, &r, &o); int z = 1, cf = 1; for (int i = 0; i < 16; i++) { if (r.a[i] & r.b[i]) z = 0; if ((~r.a[i]) & r.b[i]) cf = 0; } total++; if (o.x[1] != (uint64_t)((z ? FL_ZF : 0) | (cf ? FL_CF : 0))) { if (bad++ < 12) printf("ptest flags\n"); } }
    }
    printf("validate-iarith: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
