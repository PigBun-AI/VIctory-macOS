// Checks isa-model-logic.h against arm64 hardware (NEON vandq/vbicq/vorrq/veorq): clang -O1 -I.. validate-logic.c -o v && ./v
#include <arm_neon.h>
#include "../isa-model-logic.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
int main(void) {
    long bad = 0, total = 0;
    for (long it = 0; it < 20000000; it++) {
        IRec r; for (int i = 0; i < 128; i++) ((uint8_t *)&r)[i] = (uint8_t)rnd(); Op op; memset(&op, 0, sizeof op); Ctx cx = {&op, 32, 0, 0};
        for (int k = LG_AND; k <= LG_XOR; k++) for (int w = 16; w <= 32; w += 16) {
            op.p = (void *)(intptr_t)k; op.w = w; cx.w = w; Out o; memset(&o, 0xAA, sizeof o); m_logic(&cx, &r, &o);
            uint8_t e[32] = {0}; for (int h = 0; h < w / 16; h++) { uint8x16_t a = vld1q_u8(r.a + 16 * h), b = vld1q_u8(r.b + 16 * h), v;
                v = k == LG_AND ? vandq_u8(a, b) : k == LG_ANDN ? vbicq_u8(b, a) : k == LG_OR ? vorrq_u8(a, b) : veorq_u8(a, b); vst1q_u8(e + 16 * h, v); }
            total++; if (memcmp(o.r, e, 32)) bad++;
        }
    }
    printf("validate-logic: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
