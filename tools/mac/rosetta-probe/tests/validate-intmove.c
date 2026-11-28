// Checks isa-model-intmove.h against arm64 hardware: the 16 x86 condition codes against Arm condition codes evaluated on hardware flags (x86 CF = !C after a
// subtraction; PF has no Arm counterpart and is checked against popcount), and mov/movzx/movsx/xchg/cmov/setcc/lea results against a byte-array reference
// (partial-register merge, 32-bit zero-extension) built independently of the model:  clang -O1 -I.. validate-intmove.c -o v && ./v
#include "../isa-model-intmove.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static uint64_t gen(void) { uint64_t x = rnd(); switch (rnd() % 8) { case 0: x = 0; break; case 1: x = ~0ull; break; case 2: x = 1ull << 63; break; case 3: x = (1ull << 63) - 1; break; case 4: x &= 0xFF; break; case 5: x &= 0xFFFFFFFF; break; default: break; } return x; }
static long bad, total;
#define ARMCC(CC) __asm__("subs xzr, %1, %2\n cset %0, " #CC : "=r"(r) : "r"(a), "r"(b) : "cc"); break
static int arm_cond(int cc, uint64_t a, uint64_t b) {
    uint64_t r = 0;
    switch (cc) { case 0: ARMCC(vs); case 1: ARMCC(vc); case 2: ARMCC(cc); case 3: ARMCC(cs); case 4: ARMCC(eq); case 5: ARMCC(ne); case 6: ARMCC(ls); case 7: ARMCC(hi); case 8: ARMCC(mi); case 9: ARMCC(pl);
    case 12: ARMCC(lt); case 13: ARMCC(ge); case 14: ARMCC(le); case 15: ARMCC(gt); }
    return (int)r;
}
// x86 flags of a - b computed from hardware NZCV
static uint32_t flags_of_sub(uint64_t a, uint64_t b) {
    uint64_t res, nzcv; __asm__("subs %1, %2, %3\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(a), "r"(b) : "cc");
    uint32_t f = 0; if (!((nzcv >> 29) & 1)) f |= FL_CF; if ((nzcv >> 30) & 1) f |= FL_ZF; if ((nzcv >> 31) & 1) f |= FL_SF; if ((nzcv >> 28) & 1) f |= FL_OF;
    if (__builtin_popcount((uint32_t)(res & 0xFF)) % 2 == 0) f |= FL_PF; return f;
}
// reference register file as bytes
static void ref_write(uint8_t reg[8], int wb, uint64_t v) { for (int i = 0; i < wb; i++) reg[i] = (uint8_t)(v >> (8 * i)); if (wb == 4) for (int i = 4; i < 8; i++) reg[i] = 0; }
static uint64_t ref_u64(const uint8_t reg[8]) { uint64_t v = 0; for (int i = 7; i >= 0; i--) v = (v << 8) | reg[i]; return v; }
static void chk(const char *w, uint64_t got, uint64_t want) { total++; if (got != want) { if (bad++ < 10) printf("%s got=%llx want=%llx\n", w, (unsigned long long)got, (unsigned long long)want); } }
int main(void) {
    for (long it = 0; it < 4000000; it++) {
        uint64_t a = gen(), b = gen(); if (rnd() % 4 == 0) b = a + (rnd() % 3) - 1;
        uint32_t f = flags_of_sub(a, b);
        for (int cc = 0; cc < 16; cc++) { int want = cc == 10 ? !!(f & FL_PF) : cc == 11 ? !(f & FL_PF) : arm_cond(cc, a, b); chk("cc", (uint64_t)cc_true(cc, f), (uint64_t)want); }
        // arbitrary flags: parity and the p/np pair, the other conditions as boolean formulas of the six flags
        uint32_t g = (uint32_t)rnd() & FL_ALL; for (int cc = 0; cc < 16; cc++) {
            int o = !!(g & FL_OF), cfl = !!(g & FL_CF), z = !!(g & FL_ZF), sg = !!(g & FL_SF), p = !!(g & FL_PF), w = 0;
            switch (cc) { case 0: w = o; break; case 1: w = !o; break; case 2: w = cfl; break; case 3: w = !cfl; break; case 4: w = z; break; case 5: w = !z; break; case 6: w = cfl || z; break; case 7: w = !cfl && !z; break;
            case 8: w = sg; break; case 9: w = !sg; break; case 10: w = p; break; case 11: w = !p; break; case 12: w = sg != o; break; case 13: w = sg == o; break; case 14: w = z || sg != o; break; default: w = !z && sg == o; }
            chk("cc2", (uint64_t)cc_true(cc, g), (uint64_t)w); }
        // moves through the model vs a byte-array reference
        Op op; memset(&op, 0, sizeof op); Ctx cx = {&op, 32, 0, 0}; IRec r; for (int i = 0; i < 128; i++) ((uint8_t *)&r)[i] = (uint8_t)rnd();
        st_(r.a, 8, a); st_(r.b, 8, b); st_(r.c + 8, 8, g | 2); static const int wbs[4] = {1, 2, 4, 8}; int wb = wbs[rnd() % 4]; Out o;
        uint8_t ra[8], rb[8]; for (int i = 0; i < 8; i++) { ra[i] = (uint8_t)(a >> (8 * i)); rb[i] = (uint8_t)(b >> (8 * i)); }
        // mov rax <- rbx
        op.p = (void *)(intptr_t)(MV_MOV | (wb << 8) | (0 << 16) | (1 << 20)); memset(&o, 0, sizeof o); m_intmove(&cx, &r, &o); { uint8_t t[8]; memcpy(t, ra, 8); ref_write(t, wb, ld_(rb, wb)); chk("mov", o.x[0], ref_u64(t)); chk("mov flags", o.x[1], g & FL_ALL); chk("mov rbx", o.x[3], b); }
        // movzx / movsx from rbx(low sw bytes) to rax(wb)
        int sw = wbs[rnd() % 4]; if (sw < wb) {
            op.p = (void *)(intptr_t)(MV_MOVZX | (wb << 8) | (sw << 12) | (0 << 16) | (1 << 20)); memset(&o, 0, sizeof o); m_intmove(&cx, &r, &o);
            { uint8_t t[8]; memcpy(t, ra, 8); ref_write(t, wb, ld_(rb, sw)); chk("movzx", o.x[0], ref_u64(t)); }
            op.p = (void *)(intptr_t)(MV_MOVSX | (wb << 8) | (sw << 12) | (0 << 16) | (1 << 20)); memset(&o, 0, sizeof o); m_intmove(&cx, &r, &o);
            { uint8_t t[8]; memcpy(t, ra, 8); int64_t sv = sw == 1 ? (int8_t)rb[0] : sw == 2 ? (int16_t)(rb[0] | (rb[1] << 8)) : (int32_t)ld_(rb, 4); ref_write(t, wb, (uint64_t)sv); chk("movsx", o.x[0], ref_u64(t)); } }
        // cmov: dst rax, src rbx
        int cc = (int)(rnd() % 16); if (wb > 1) { op.p = (void *)(intptr_t)(MV_CMOV | (wb << 8) | (0 << 16) | (1 << 20) | (cc << 24)); memset(&o, 0, sizeof o); m_intmove(&cx, &r, &o);
            uint8_t t[8]; memcpy(t, ra, 8); if (cc_true(cc, g & FL_ALL)) ref_write(t, wb, ld_(rb, wb)); else if (wb == 4) ref_write(t, 4, ld_(ra, 4)); chk("cmov", o.x[0], ref_u64(t)); }
        // setcc
        op.p = (void *)(intptr_t)(MV_SETCC | (1 << 8) | (0 << 16) | (3 << 20) | (cc << 24)); memset(&o, 0, sizeof o); m_intmove(&cx, &r, &o); { uint8_t t[8]; memcpy(t, ra, 8); t[0] = (uint8_t)cc_true(cc, g & FL_ALL); chk("setcc", o.x[0], ref_u64(t)); }
        // xchg rax <-> rbx
        op.p = (void *)(intptr_t)(MV_XCHG | (wb << 8) | (0 << 16) | (1 << 20)); memset(&o, 0, sizeof o); m_intmove(&cx, &r, &o); { uint8_t x[8], y[8]; memcpy(x, ra, 8); memcpy(y, rb, 8); uint8_t tx[8], ty[8]; memcpy(tx, ra, 8); memcpy(ty, rb, 8);
            ref_write(tx, wb, ld_(rb, wb)); ref_write(ty, wb, ld_(ra, wb)); chk("xchg a", o.x[0], ref_u64(tx)); chk("xchg b", o.x[3], ref_u64(ty)); }
        // lea
        { int sc = (int[]){0, 1, 2, 4, 8}[rnd() % 5], base = rnd() & 1, a32 = rnd() & 1; int disp = (int)rnd(); if (!base && !sc) sc = 1;
          op.p = (void *)(intptr_t)(MV_LEA | (wb << 8) | (sc << 12) | (3 << 16) | ((base | (a32 << 1)) << 24)); op.imm = disp; memset(&o, 0, sizeof o); m_intmove(&cx, &r, &o);
          unsigned __int128 ea = 0; if (base) ea += a32 ? (uint32_t)a : a; if (sc) ea += (unsigned __int128)(a32 ? (uint64_t)(uint32_t)b : b) * (unsigned)sc; ea += (unsigned __int128)(int64_t)disp; uint64_t e = (uint64_t)ea; if (a32) e &= 0xFFFFFFFFull;
          uint8_t t[8]; for (int i = 0; i < 8; i++) t[i] = (uint8_t)(ld_(r.d, 8) >> (8 * i)); ref_write(t, wb == 2 ? 2 : wb, e); chk("lea", o.x[2], ref_u64(t)); }
    }
    printf("validate-intmove: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
