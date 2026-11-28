// Family avx_logic: vandps/vandpd/vandnps/vandnpd/vorps/vorpd/vxorps/vxorpd and vpand/vpandn/vpor/vpxor, xmm and ymm, register, memory and the alias forms
// (vxorps x,x,x is the zeroing idiom the title uses 146,289 times). Bitwise ops are independent per bit, so the exhaustive pass over all 65,536 byte pairs in
// every lane position covers the whole truth table of every lane.
#ifndef ISA_LOGIC_H
#define ISA_LOGIC_H
#include "isa-model-logic.h"
#define LG(M) G3(M)
LG(vandps) LG(vandpd) LG(vandnps) LG(vandnpd) LG(vorps) LG(vorpd) LG(vxorps) LG(vxorpd) LG(vpand) LG(vpandn) LG(vpor) LG(vpxor)
#define RLG(M, K) REG3(M, m_logic, EK_I8, (void *)(intptr_t)K, 0, 2, 1)
static void reg_logic_ops(void) {
    RLG(vandps, LG_AND) RLG(vandpd, LG_AND) RLG(vandnps, LG_ANDN) RLG(vandnpd, LG_ANDN) RLG(vorps, LG_OR) RLG(vorpd, LG_OR) RLG(vxorps, LG_XOR) RLG(vxorpd, LG_XOR)
    RLG(vpand, LG_AND) RLG(vpandn, LG_ANDN) RLG(vpor, LG_OR) RLG(vpxor, LG_XOR)
}
#endif
