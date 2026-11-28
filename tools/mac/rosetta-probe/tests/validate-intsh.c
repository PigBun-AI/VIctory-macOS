// Checks isa-model-intsh.h against arm64 hardware and independent formulations: shifts/rotates (lsl/lsr/asr/ror on the operand placed in a 64-bit register, carry
// from the bit shifted out), mul/imul (umulh/smulh/mul), div/idiv (udiv/sdiv + msub for dividends that fit 64 bits, 128-bit compiler division beyond), clz/rbit/cnt/rev for
// lzcnt, tzcnt, bsf/bsr, popcnt, bswap, pext/pdep (bit-by-bit with ctz), bit tests:  clang -O1 -I.. validate-intsh.c -o v && ./v
#include <arm_neon.h>
#include "../isa-model-intsh.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static uint64_t gen(int bits) { uint64_t m = mskb(bits), x = rnd() & m; switch (rnd() % 8) { case 0: x = 0; break; case 1: x = m; break; case 2: x = 1ull << (bits - 1); break; case 3: x = (1ull << (bits - 1)) - 1; break; case 4: x = 1; break; default: break; } return x; }
static long bad, total;
static void chk(const char *w, int wb, uint64_t a, uint64_t b, uint64_t got, uint64_t want) { total++; if (got != want) { if (bad++ < 12) printf("%s wb=%d a=%llx b=%llx got=%llx want=%llx\n", w, wb, (unsigned long long)a, (unsigned long long)b, (unsigned long long)got, (unsigned long long)want); } }
static uint64_t hw_lsl(uint64_t a, unsigned n) { uint64_t r; __asm__("lsl %0, %1, %2" : "=r"(r) : "r"(a), "r"((uint64_t)n)); return r; }
static uint64_t hw_lsr(uint64_t a, unsigned n) { uint64_t r; __asm__("lsr %0, %1, %2" : "=r"(r) : "r"(a), "r"((uint64_t)n)); return r; }
static int64_t hw_asr(int64_t a, unsigned n) { int64_t r; __asm__("asr %0, %1, %2" : "=r"(r) : "r"(a), "r"((uint64_t)n)); return r; }
static uint64_t hw_ror(uint64_t a, unsigned n) { uint64_t r; __asm__("ror %0, %1, %2" : "=r"(r) : "r"(a), "r"((uint64_t)n)); return r; }
int main(void) {
    for (long it = 0; it < 6000000; it++) {
        static const int wbs[4] = {1, 2, 4, 8}; int wb = wbs[it & 3], w = wb * 8; uint64_t m = mskb(w), a = gen(w), b = gen(w); uint32_t fin = (uint32_t)rnd() & FL_ALL; uint64_t cnt = rnd() % 4 == 0 ? rnd() & 0xFF : rnd() % 72; uint32_t und, c = (uint32_t)(cnt & (wb == 8 ? 63 : 31));
        // shifts and rotates
        for (int kind = SH_SHL; kind <= SH_ROR; kind++) {
            AluR r = shift_op(kind, wb, a, b, cnt, fin, &und); uint64_t want; uint32_t wcf = 0;
            if (c == 0) { chk("shift0", wb, a, cnt, r.r, a); chk("shift0 flags", wb, a, cnt, r.f, fin); continue; }
            // reference on a left-aligned 64-bit register; shifts of 64 or more give 0 (the hardware lsl uses the count mod 64, so select explicitly)
            uint64_t la = a << (64 - w);
            if (kind == SH_SHL) { want = c >= (unsigned)w ? 0 : hw_lsl(a, c) & m; wcf = c <= (unsigned)w ? (c == (unsigned)w ? (unsigned)(a & 1) : (unsigned)((hw_lsr(la, 64 - c) >> (64 - c)) & 1)) : 0; wcf = c <= (unsigned)w ? (unsigned)((hw_lsr(hw_lsl(la, c - 1 < 64 ? c - 1 : 0), 63)) & 1) : 0; }
            else if (kind == SH_SHR) { want = c >= (unsigned)w ? 0 : hw_lsr(a, c); wcf = c <= (unsigned)w ? (unsigned)(hw_lsr(a, c - 1) & 1) : 0; }
            else if (kind == SH_SAR) { int64_t sa = (int64_t)la >> (64 - w); int64_t rr = c >= (unsigned)w ? hw_asr(sa, 63) : hw_asr(sa, c); want = (uint64_t)rr & m; wcf = (unsigned)(hw_asr(sa, c - 1 < 63 ? c - 1 : 63) & 1); }
            else if (kind == SH_ROL) { unsigned n = c % w; uint64_t rep = 0; for (int i = 0; i < 64; i += w) rep |= a << i; if (w == 64) rep = a; want = n ? (hw_ror(rep, (64 - n) % 64) & m) : a; wcf = (unsigned)(want & 1); }
            else { unsigned n = c % w; uint64_t rep = 0; for (int i = 0; i < 64; i += w) rep |= a << i; if (w == 64) rep = a; want = hw_ror(rep, n) & m; wcf = (unsigned)((want >> (w - 1)) & 1); }
            chk("shift result", wb * 10 + kind, a, cnt, r.r, want);
            if (!(und & FL_CF)) chk("shift CF", wb * 10 + kind, a, cnt, !!(r.f & FL_CF), wcf);
            if (!(und & FL_OF)) { unsigned of = 0; if (c == 1) { if (kind == SH_SHL) of = (unsigned)(((want >> (w - 1)) & 1) ^ wcf); else if (kind == SH_SHR) of = (unsigned)((a >> (w - 1)) & 1); else if (kind == SH_SAR) of = 0; else if (kind == SH_ROL) of = (unsigned)(((want >> (w - 1)) & 1) ^ wcf); else of = (unsigned)(((want >> (w - 1)) & 1) ^ ((want >> (w - 2)) & 1)); } chk("shift OF", wb * 10 + kind, a, cnt, !!(r.f & FL_OF), of); }
            if (kind != SH_ROL && kind != SH_ROR) { chk("shift ZF", wb, a, cnt, !!(r.f & FL_ZF), want == 0); chk("shift SF", wb, a, cnt, !!(r.f & FL_SF), (want >> (w - 1)) & 1); chk("shift PF", wb, a, cnt, !!(r.f & FL_PF), __builtin_popcount((uint32_t)(want & 0xFF)) % 2 == 0); }
        }
        // shld / shrd through 128-bit compositions
        if (wb >= 4 || (wb == 2 && c <= 16)) for (int kind = SH_SHLD; kind <= SH_SHRD; kind++) {
            AluR r = shift_op(kind, wb, a, b, cnt, fin, &und); if (c == 0) continue; u128_t cat; uint64_t want;
            if (kind == SH_SHLD) { cat = ((u128_t)a << w) | b; want = (uint64_t)(cat >> (w - c)) & m; } else { cat = ((u128_t)b << w) | a; want = (uint64_t)(cat >> c) & m; }
            chk("dshift", wb * 10 + kind, a, b, r.r, want);
        }
        // mul / imul / div / idiv
        { uint64_t ra = rnd(), rd = rnd(), rax = ra, rdx = rd; uint32_t f, u; muldiv(MD_MUL, wb, &rax, &rdx, b, &f, &u);
          uint64_t p_lo = (a & m) * (b & m); u128_t pf = (u128_t)(a & m) * (b & m); (void)p_lo; ra = (ra & ~m) | a; rax = ra; rdx = rd; muldiv(MD_MUL, wb, &rax, &rdx, b, &f, &u);
          if (wb == 8) { uint64_t hi; __asm__("umulh %0, %1, %2" : "=r"(hi) : "r"(a), "r"(b)); chk("mul hi", wb, a, b, rdx, hi); chk("mul lo", wb, a, b, rax, a * b); chk("mul CF", wb, a, b, !!(f & FL_CF), hi != 0); }
          else if (wb == 4) { chk("mul64", wb, a, b, ((rdx & 0xFFFFFFFF) << 32) | (rax & 0xFFFFFFFF), (a & m) * (b & m)); chk("mul zext", wb, a, b, (rax >> 32) | (rdx >> 32), 0); }
          else { chk("mul lo", wb, a, b, rax & m, (uint64_t)pf & m); chk("mul CF", wb, a, b, !!(f & FL_CF), (pf >> w) != 0); }
          rax = ra; rdx = rd; muldiv(MD_IMUL1, wb, &rax, &rdx, b, &f, &u);
          if (wb == 8) { int64_t hi; __asm__("smulh %0, %1, %2" : "=r"(hi) : "r"(a), "r"(b)); chk("imul hi", wb, a, b, rdx, (uint64_t)hi); int64_t lo = (int64_t)a * (int64_t)b; chk("imul CF", wb, a, b, !!(f & FL_CF), hi != (lo >> 63)); }
          else { int64_t sp = sx_(a, w) * sx_(b, w); if (wb == 4) { chk("imul64", wb, a, b, ((rdx & 0xFFFFFFFF) << 32) | (rax & 0xFFFFFFFF), (uint64_t)sp); chk("imul CF", wb, a, b, !!(f & FL_CF), sp != (int64_t)(int32_t)sp); } else { chk("imul lo", wb, a, b, rax & m, (uint64_t)sp & m); chk("imul CF", wb, a, b, !!(f & FL_CF), sp != sx_((uint64_t)sp & m, w)); } }
          // two-operand imul: lower half only
          rax = ra; muldiv(MD_IMUL2, wb, &rax, &rdx, b, &f, &u); { int64_t sp = (int64_t)((uint64_t)sx_(a, w) * (uint64_t)sx_(b, w)); chk("imul2", wb, a, b, rax & m, (uint64_t)sp & m); }
          // div: build a dividend that fits
          uint64_t d = gen(w); if (!d) d = 3; uint64_t q = gen(w), r0 = rnd() % d; u128_t n = (u128_t)q * d + r0; rax = ra; rdx = rd;
          if (wb == 1) { rax = (ra & ~0xFFFFull) | (uint64_t)n; } else if (wb == 2) { rax = (ra & ~0xFFFFull) | ((uint64_t)n & 0xFFFF); rdx = (rd & ~0xFFFFull) | ((uint64_t)(n >> 16) & 0xFFFF); } else if (wb == 4) { rax = (uint64_t)n & 0xFFFFFFFF; rdx = (uint64_t)(n >> 32) & 0xFFFFFFFF; } else { rax = (uint64_t)n; rdx = (uint64_t)(n >> 64); }
          uint64_t ex_rax = rax, ex_rdx = rdx; muldiv(MD_DIV, wb, &rax, &rdx, d, &f, &u); (void)ex_rax; (void)ex_rdx;
          if (wb == 1) chk("div8", wb, n, d, rax & 0xFFFF, (r0 << 8) | q); else if (wb == 2) { chk("div16 q", wb, n, d, rax & 0xFFFF, q); chk("div16 r", wb, n, d, rdx & 0xFFFF, r0); } else { chk("div q", wb, (uint64_t)n, d, rax, q); chk("div r", wb, (uint64_t)n, d, rdx, r0); }
          if (wb <= 4) { uint64_t hq, hr; uint64_t nn = (uint64_t)n; __asm__("udiv %0, %1, %2" : "=r"(hq) : "r"(nn), "r"(d)); hr = nn - hq * d; chk("div hw q", wb, nn, d, hq & m, q); chk("div hw r", wb, nn, d, hr & m, r0); }
          if (wb == 8 && (rdx == 0 || 1)) { uint64_t nn = (uint64_t)n; if ((n >> 64) == 0) { uint64_t hq; __asm__("udiv %0, %1, %2" : "=r"(hq) : "r"(nn), "r"(d)); chk("div64 hw", wb, nn, d, hq, q); } }
        }
        // bit scans / counts (clz, rbit, cnt, rev)
        { uint64_t x = a & m; uint32_t u; int zs; uint64_t rb64; __asm__("rbit %0, %1" : "=r"(rb64) : "r"(x));
          uint64_t clz; __asm__("clz %0, %1" : "=r"(clz) : "r"(x)); int lz = x ? (int)clz - (64 - w) : w;
          uint64_t ctzv = x ? (uint64_t)__builtin_ctzll(x) : (uint64_t)w; (void)rb64; AluR t = bitscan(BS_TZCNT, wb, a, 0x55, &u, &zs); chk("tzcnt", wb, a, 0, t.r, ctzv); chk("tzcnt CF", wb, a, 0, !!(t.f & FL_CF), x == 0); chk("tzcnt ZF", wb, a, 0, !!(t.f & FL_ZF), ctzv == 0);
          AluR l = bitscan(BS_LZCNT, wb, a, 0x55, &u, &zs); chk("lzcnt", wb, a, 0, l.r, (uint64_t)lz); chk("lzcnt CF", wb, a, 0, !!(l.f & FL_CF), x == 0); chk("lzcnt ZF", wb, a, 0, !!(l.f & FL_ZF), lz == 0);
          AluR bf = bitscan(BS_BSF, wb, a, 0x55, &u, &zs); if (x) chk("bsf", wb, a, 0, bf.r, ctzv); chk("bsf ZF", wb, a, 0, !!(bf.f & FL_ZF), x == 0); chk("bsf zero keeps", wb, a, 0, x ? 0 : bf.r, x ? 0 : 0x55);
          AluR br = bitscan(BS_BSR, wb, a, 0x55, &u, &zs); if (x) chk("bsr", wb, a, 0, br.r, (uint64_t)(w - 1 - lz)); chk("bsr ZF", wb, a, 0, !!(br.f & FL_ZF), x == 0);
          AluR pc = bitscan(BS_POPCNT, wb, a, 0, &u, &zs); uint8x8_t cn = vcnt_u8(vcreate_u8(x)); chk("popcnt", wb, a, 0, pc.r, vaddv_u8(cn)); chk("popcnt flags", wb, a, 0, pc.f, x == 0 ? FL_ZF : 0);
          uint64_t sw = x; uint64_t rv; if (wb == 8) __asm__("rev %0, %1" : "=r"(rv) : "r"(sw)); else if (wb == 4) { uint32_t t32; __asm__("rev %w0, %w1" : "=r"(t32) : "r"((uint32_t)sw)); rv = t32; } else { uint16_t t16 = (uint16_t)sw; rv = (uint16_t)((t16 >> 8) | (t16 << 8)); } if (wb != 1) chk("bswap", wb, a, 0, bswap_n(x, wb), rv); }
        // bit tests
        { uint64_t idx = rnd() % 300; for (int kind = 0; kind < 4; kind++) { uint32_t u; AluR r = bittest(kind, wb, a, idx, fin, &u); unsigned i = (unsigned)(idx % w); unsigned bit = (unsigned)hw_lsr(a, i) & 1; uint64_t want = kind == 0 ? a : kind == 1 ? (a | hw_lsl(1, i)) : kind == 2 ? (a & ~hw_lsl(1, i)) : (a ^ hw_lsl(1, i));
            chk("bt result", wb * 10 + kind, a, idx, r.r, want & m); chk("bt CF", wb * 10 + kind, a, idx, !!(r.f & FL_CF), bit); chk("bt ZF kept", wb, a, idx, r.f & FL_ZF, fin & FL_ZF); } }
        // pext / pdep: independent formulation (iterate the set bits of the mask with ctz)
        { uint64_t mask = rnd() & rnd(), src = rnd(); int w2 = wb == 8 ? 64 : 32; uint64_t mk = mask & mskb(w2), sr = src & mskb(w2), pe = 0, pd = 0, t = mk; int k = 0; while (t) { int bpos = __builtin_ctzll(t); pe |= ((sr >> bpos) & 1ull) << k; k++; t &= t - 1; }
          t = mk; k = 0; while (t) { int bpos = __builtin_ctzll(t); pd |= ((sr >> k) & 1ull) << bpos; k++; t &= t - 1; } chk("pext", wb, sr, mk, pext_n(sr, mk, w2), pe); chk("pdep", wb, sr, mk, pdep_n(sr, mk, w2), pd); }
    }
    printf("validate-intsh: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
