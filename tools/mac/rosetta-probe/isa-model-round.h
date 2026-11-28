// Integer-only model of round-to-integral (vroundps/pd/ss/sd): imm bits 0-1 select the rounding, bit 2 selects the MXCSR mode.
#ifndef ISA_MODEL_ROUND_H
#define ISA_MODEL_ROUND_H
#include "isa-model-cvt.h"
static uint64_t rnd_int(Fmt f, uint64_t x, int imm, Fpc c) {
    if (sf_isnan(f, x)) return fp_quiet(f, x);
    if (c.daz) x = sf_daz(f, x);
    Unp u = sf_unpack(f, x);
    if (u.cls != 1 || u.e >= 0) return x;
    int rm = (imm & 4) ? c.rm : (imm & 3), sh = -u.e; u128 kept; int rbit = 0, stk = 0;
    if (sh >= 127) { kept = 0; stk = 1; } else { kept = (u128)u.sig >> sh; u128 m = (u128)u.sig & (((u128)1 << sh) - 1); rbit = (int)((m >> (sh - 1)) & 1); stk = (m & (((u128)1 << (sh - 1)) - 1)) != 0; }
    int inexact = rbit || stk, up = 0;
    switch (rm) { case 0: up = rbit && (stk || (int)(kept & 1)); break; case 1: up = u.sign && inexact; break; case 2: up = !u.sign && inexact; break; default: up = 0; }
    kept += (u128)up;
    if (kept == 0) return (uint64_t)u.sign << (f.eb + f.mb);
    return sf_round(f, u.sign, kept, 0, 0, 0);
}
#endif
