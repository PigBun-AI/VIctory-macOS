// Software model of the x86 shift / rotate / double shift / multiply / divide / bit scan / bit test / BMI instructions with their flags.
// Flags the SDM calls undefined are reported through *_und() masks and excluded from the comparison (per record: OF after a multi-bit shift, AF after any shift,
// CF after shl/shr by >= the width, SF ZF AF PF after mul/imul, everything after div/idiv, ...). Portable C (validated on arm64 by tests/validate-intsh.c).
#ifndef ISA_MODEL_INTSH_H
#define ISA_MODEL_INTSH_H
#include "isa-model-intmove.h"
typedef unsigned __int128 u128_t; typedef __int128 s128_t;
enum { SH_SHL, SH_SHR, SH_SAR, SH_ROL, SH_ROR, SH_SHLD, SH_SHRD, SH_RCL, SH_RCR };
static inline uint32_t sh_cmask(int wb, uint64_t cnt) { return (uint32_t)(cnt & (wb == 8 ? 63 : 31)); }
// result and flags of a shift/rotate; a = destination operand, b = second operand of shld/shrd; fin = initial flags; *und = flags to ignore
static AluR shift_op(int kind, int wb, uint64_t a, uint64_t b, uint64_t cntraw, uint32_t fin, uint32_t *und) {
    int w = wb * 8; uint64_t m = mskb(w), top = 1ull << (w - 1); a &= m; b &= m; uint32_t c = sh_cmask(wb, cntraw); AluR o = {a, fin & FL_ALL}; *und = 0;
    if (c == 0) return o;
    if (MODEL_NEG && kind == SH_SAR && c >= (uint32_t)w) c %= (uint32_t)w;      // negative control: the count taken modulo the width
    uint64_t r = a; uint32_t f = 0; int cf = 0, of = 0;
    switch (kind) {
    case SH_SHL: r = c >= (uint32_t)w ? 0 : (a << c) & m; cf = c <= (uint32_t)w ? (c == (uint32_t)w ? (a & 1) : (a >> (w - c)) & 1) : 0; of = !!(r & top) ^ cf; break;
    case SH_SHR: r = c >= (uint32_t)w ? 0 : a >> c; cf = c <= (uint32_t)w ? (a >> (c - 1)) & 1 : 0; of = !!(a & top); break;
    case SH_SAR: { int64_t sa = sx_(a, w); r = (uint64_t)(c >= (uint32_t)w ? (sa < 0 ? -1 : 0) : sa >> c) & m; cf = (uint64_t)(c >= (uint32_t)w ? (sa < 0) : ((sa >> (c - 1)) & 1)); of = 0; break; }
    case SH_ROL: { uint32_t n = c % (uint32_t)w; r = n ? ((a << n) | (a >> (w - n))) & m : a; cf = (int)(r & 1); of = !!(r & top) ^ cf; break; }
    case SH_ROR: { uint32_t n = c % (uint32_t)w; r = n ? ((a >> n) | (a << (w - n))) & m : a; cf = !!(r & top); of = !!(r & top) ^ !!(r & (top >> 1)); break; }
    case SH_SHLD: r = ((a << c) | (b >> (w - c))) & m; cf = (int)((a >> (w - c)) & 1); of = !!(r & top) ^ !!(a & top); break;
    case SH_SHRD: r = ((a >> c) | (b << (w - c))) & m; cf = (int)((a >> (c - 1)) & 1); of = !!(r & top) ^ !!(a & top); break;
    }
    f = (cf ? FL_CF : 0) | (of ? FL_OF : 0);
    if (kind == SH_ROL || kind == SH_ROR) { f |= fin & (FL_SF | FL_ZF | FL_AF | FL_PF); }       // rotates leave SF ZF AF PF alone
    else f |= fl_szp(r, w);
    if (c != 1) *und |= FL_OF;
    if (kind != SH_ROL && kind != SH_ROR) *und |= FL_AF;
    if ((kind == SH_SHL || kind == SH_SHR) && c >= (uint32_t)w) *und |= FL_CF;
    o.r = r; o.f = f; return o;
}
enum { MD_MUL, MD_IMUL1, MD_IMUL2, MD_IMUL3, MD_DIV, MD_IDIV };
// mul / imul / div / idiv: rax, rdx in/out.  Returns through the pointers; dividend of div = rdx:rax (ax for 8 bit), divisor = s
static void muldiv(int kind, int wb, uint64_t *rax, uint64_t *rdx, uint64_t s, uint32_t *f, uint32_t *und) {
    int w = wb * 8; uint64_t m = mskb(w), a = *rax & m; s &= m; *und = FL_SF | FL_ZF | FL_AF | FL_PF; *f = 0;
    if (kind == MD_MUL || kind == MD_IMUL1) {
        u128_t p; int ovf;
        if (kind == MD_MUL) { p = (u128_t)a * s; ovf = (p >> w) != 0; }
        else { s128_t sp = (s128_t)sx_(a, w) * (s128_t)sx_(s, w); p = (u128_t)sp; ovf = sp != (s128_t)sx_((uint64_t)p & m, w); }
        uint64_t lo = (uint64_t)p & m, hi = (uint64_t)(p >> w) & m;
        if (wb == 1) *rax = (*rax & ~0xFFFFull) | (uint64_t)(p & 0xFFFF);
        else if (wb == 2) { *rax = (*rax & ~0xFFFFull) | lo; *rdx = (*rdx & ~0xFFFFull) | hi; }
        else { *rax = lo; *rdx = hi; }
        *f = ovf ? (FL_CF | FL_OF) : 0; return;
    }
    if (kind == MD_IMUL2 || kind == MD_IMUL3) {      // rax = a * s: here a and s are both operands (the caller passes them); destination width wb
        s128_t sp = (s128_t)sx_(a, w) * (s128_t)sx_(s, w); uint64_t lo = (uint64_t)sp & m; int ovf = sp != (s128_t)sx_(lo, w);
        *rax = wb >= 4 ? lo : (*rax & ~m) | lo; if (wb == 4) *rax = lo; *f = ovf ? (FL_CF | FL_OF) : 0; return;
    }
    // div / idiv
    *und = FL_ALL; uint64_t q, r;
    if (kind == MD_DIV) {
        u128_t n = wb == 1 ? (u128_t)(*rax & 0xFFFF) : wb == 2 ? (((u128_t)(*rdx & 0xFFFF) << 16) | (*rax & 0xFFFF)) : wb == 4 ? (((u128_t)(*rdx & 0xFFFFFFFFull) << 32) | (*rax & 0xFFFFFFFFull)) : (((u128_t)*rdx << 64) | *rax);
        q = (uint64_t)(n / s); r = (uint64_t)(n % s);
    } else {
        s128_t n = wb == 1 ? (s128_t)(int16_t)*rax : wb == 2 ? (s128_t)(int32_t)(uint32_t)(((*rdx & 0xFFFF) << 16) | (*rax & 0xFFFF)) : wb == 4 ? (s128_t)(int64_t)(((*rdx & 0xFFFFFFFFull) << 32) | (*rax & 0xFFFFFFFFull)) : (s128_t)(((u128_t)*rdx << 64) | *rax);
        s128_t d = (s128_t)sx_(s, w); s128_t qq = n / d, rr = n % d; q = (uint64_t)qq; r = (uint64_t)rr;
    }
    q &= m; r &= m;
    if (wb == 1) *rax = (*rax & ~0xFFFFull) | (r << 8) | q;
    else if (wb == 2) { *rax = (*rax & ~0xFFFFull) | q; *rdx = (*rdx & ~0xFFFFull) | r; }
    else { *rax = q; *rdx = r; }
}
// bit scan / count: returns result in *r (dst), flags; *und = undefined flags; *zsrc = 1 when the destination is architecturally undefined (bsf/bsr of zero)
enum { BS_BSF, BS_BSR, BS_TZCNT, BS_LZCNT, BS_POPCNT };
static AluR bitscan(int kind, int wb, uint64_t src, uint64_t dstold, uint32_t *und, int *zsrc) {
    int w = wb * 8; uint64_t m = mskb(w); src &= m; uint64_t r = 0; uint32_t f = 0; *und = 0; *zsrc = 0; AluR o;
    switch (kind) {
    case BS_BSF: if (!src && MODEL_NEG) { r = (uint64_t)w; f = FL_ZF; *zsrc = 0; } else if (!src) { r = dstold; f = FL_ZF; *zsrc = 1; } else { r = 0; while (!((src >> r) & 1)) r++; } *und = FL_ALL & ~FL_ZF; if (src) f = 0; break;
    case BS_BSR: if (!src) { r = dstold; f = FL_ZF; *zsrc = 1; } else { r = (uint64_t)(w - 1); while (!((src >> r) & 1)) r--; } *und = FL_ALL & ~FL_ZF; break;
    case BS_TZCNT: if (!src) { r = (uint64_t)w; f = FL_CF; } else { r = 0; while (!((src >> r) & 1)) r++; } if (!r) f |= FL_ZF; *und = FL_OF | FL_SF | FL_AF | FL_PF; break;
    case BS_LZCNT: if (!src) { r = (uint64_t)w; f = FL_CF; } else { r = (uint64_t)(w - 1); while (!((src >> r) & 1)) r--; r = (uint64_t)(w - 1) - r; } if (!r) f |= FL_ZF; *und = FL_OF | FL_SF | FL_AF | FL_PF; break;
    default: { r = 0; for (int i = 0; i < w; i++) r += (src >> i) & 1; if (!src) f = FL_ZF; break; }
    }
    o.r = r; o.f = f; return o;
}
// bit test family on an operand of wb bytes, bit index idx (masked by the width): kind 0 bt, 1 bts, 2 btr, 3 btc
static AluR bittest(int kind, int wb, uint64_t a, uint64_t idx, uint32_t fin, uint32_t *und) {
    int w = wb * 8; uint64_t m = mskb(w); a &= m; uint32_t i = (uint32_t)(idx % (uint64_t)w); int bit = (int)((a >> i) & 1); uint64_t r = a;
    if (kind == 1) r = a | (1ull << i); else if (kind == 2) r = a & ~(1ull << i); else if (kind == 3) r = a ^ (1ull << i);
    AluR o = {r & m, (fin & FL_ALL & ~FL_CF) | (bit ? FL_CF : 0)}; *und = FL_OF | FL_SF | FL_AF | FL_PF; return o;
}
static uint64_t bswap_n(uint64_t v, int wb) { uint64_t r = 0; for (int i = 0; i < wb; i++) r |= ((v >> (8 * i)) & 0xFF) << (8 * (wb - 1 - i)); return r; }
static uint64_t pext_n(uint64_t src, uint64_t mask, int w) { uint64_t r = 0; int k = 0; for (int i = 0; i < w; i++) if ((mask >> i) & 1) { r |= ((src >> i) & 1ull) << k; k++; } return r; }
static uint64_t pdep_n(uint64_t src, uint64_t mask, int w) { uint64_t r = 0; int k = 0; for (int i = 0; i < w; i++) if ((mask >> i) & 1) { r |= ((src >> k) & 1ull) << i; k++; } return r; }
#endif
