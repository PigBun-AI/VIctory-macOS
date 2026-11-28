// Family x86_int_move: mov/movzx/movsx/movsxd/xchg/cmovcc/setcc/lea (register, memory and immediate forms, all widths, 16 conditions x 6 flags) and the
// conditional jumps (jcc rel8 and rel32 for 16 conditions, jrcxz, jecxz). See isa-int.h for the kernel conventions.
#ifndef ISA_INTMOVE_H
#define ISA_INTMOVE_H
#include "isa-int.h"
#include "isa-model-intmove.h"
// ---- mov ----
#define MOVW(W, RA, RB, S, I3) \
    INK(k_mov_##W##_rr, "mov %%" RB ",%%" RA) INK(k_mov_##W##_rr2, "mov %%" RA ",%%" RB) INK(k_mov_##W##_rm, "mov 64(%0),%%" RA) INK(k_mov_##W##_rm3, "mov 67(%0),%%" RB) \
    INK(k_mov_##W##_mr0, "mov %%" RA ",(%1)") INK(k_mov_##W##_mr3, "mov %%" RB ",3(%1)") INK(k_mov_##W##_ri, "mov $" I3 ",%%" RA) INK(k_mov_##W##_ri2, "mov $-1,%%" RB) \
    INK(k_mov_##W##_mi, "mov" S " $0x55,3(%1)")
MOVW(8, "al", "bl", "b", "0x80") MOVW(16, "ax", "bx", "w", "0x8000") MOVW(32, "eax", "ebx", "l", "0x80000000") MOVW(64, "rax", "rbx", "q", "-2147483648")
INK(k_movabs, "movabs $0x8123456789abcdef,%%rax")
#define XEXT(NAME, MN, RA, RB, RS) INK(k_##NAME##_rr, MN " %%" RS ",%%" RA) INK(k_##NAME##_aa, MN " %%" RS ",%%" RB) INK(k_##NAME##_rm, MN " 64(%0),%%" RA) INK(k_##NAME##_rm3, MN " 67(%0),%%" RB)
XEXT(movzbw, "movzbw", "ax", "bx", "bl") XEXT(movzbl, "movzbl", "eax", "ebx", "bl") XEXT(movzbq, "movzbq", "rax", "rbx", "bl") XEXT(movzwl, "movzwl", "eax", "ebx", "bx") XEXT(movzwq, "movzwq", "rax", "rbx", "bx")
XEXT(movsbw, "movsbw", "ax", "bx", "bl") XEXT(movsbl, "movsbl", "eax", "ebx", "bl") XEXT(movsbq, "movsbq", "rax", "rbx", "bl") XEXT(movswl, "movswl", "eax", "ebx", "bx") XEXT(movswq, "movswq", "rax", "rbx", "bx") XEXT(movslq, "movslq", "rax", "rbx", "ebx")
// the source of the register forms is bl/bx/ebx; "aa" has the source register as destination (movzbl %bl,%ebx)
// ---- xchg ----
#define XCHG(W, RA, RB, S) INK(k_xchg_##W##_rr, "xchg %%" RB ",%%" RA) INK(k_xchg_##W##_aa, "xchg %%" RA ",%%" RA) INK(k_xchg_##W##_mr0, "xchg %%" RA ",(%1)") INK(k_xchg_##W##_mr3, "xchg %%" RB ",3(%1)")
XCHG(8, "al", "bl", "b") XCHG(16, "ax", "bx", "w") XCHG(32, "eax", "ebx", "l") XCHG(64, "rax", "rbx", "q")
// ---- cmovcc / setcc ----
#define CCS(X, C) \
    INK(k_cmov##C##_16_rr, "cmov" #C " %%bx,%%ax") INK(k_cmov##C##_32_rr, "cmov" #C " %%ebx,%%eax") INK(k_cmov##C##_64_rr, "cmov" #C " %%rbx,%%rax") \
    INK(k_cmov##C##_16_rm, "cmov" #C " 64(%0),%%ax") INK(k_cmov##C##_32_rm, "cmov" #C " 64(%0),%%eax") INK(k_cmov##C##_64_rm, "cmov" #C " 64(%0),%%rax") \
    INK(k_cmov##C##_32_aa, "cmov" #C " %%eax,%%eax") INK(k_cmov##C##_64_rm3, "cmov" #C " 67(%0),%%rbx") \
    INK(k_set##C##_r, "set" #C " %%al") INK(k_set##C##_r2, "set" #C " %%bl") INK(k_set##C##_m, "set" #C " 3(%1)")
#define ALLCC(X) X(o,o) X(no,no) X(b,b) X(ae,ae) X(e,e) X(ne,ne) X(be,be) X(a,a) X(s,s) X(ns,ns) X(p,p) X(np,np) X(l,l) X(ge,ge) X(le,le) X(g,g)
#define CC1(N, C) CCS(0, C)
ALLCC(CC1)
// ---- lea ----
#define LEA(NAME, EXPR) INK(k_lea_##NAME##_64, "lea " EXPR ",%%rdx") INK(k_lea_##NAME##_32, "lea " EXPR ",%%edx") INK(k_lea_##NAME##_16, "lea " EXPR ",%%dx")
LEA(b0, "(%%rax)") LEA(b7f, "0x7f(%%rax)") LEA(bd32, "0x12345678(%%rax)") LEA(bneg, "-8(%%rax)") LEA(bi1, "(%%rax,%%rbx)") LEA(bi2, "(%%rax,%%rbx,2)") LEA(bi4, "-0x10(%%rax,%%rbx,4)") LEA(bi8, "0x12345678(%%rax,%%rbx,8)")
LEA(i2, "0x40(,%%rbx,2)") LEA(i4, "(,%%rbx,4)") LEA(i8, "-3(,%%rbx,8)") LEA(abs, "0x1234(,%%rbx,1)")
LEA(a32b, "0x7f(%%eax)") LEA(a32bi, "(%%eax,%%ebx,4)") LEA(a32bi8, "-0x1000(%%eax,%%ebx,8)")
// ---- jcc ----
#define JCC(C) INK(k_j##C##_8, "mov $0,%%ecx\n j" #C " 5f\n jmp 6f\n 5: mov $1,%%ecx\n 6: mov %%rcx,%%rdx") \
    INK(k_j##C##_32, "mov $0,%%ecx\n j" #C " 5f\n jmp 6f\n .fill 140,1,0x90\n 5: mov $1,%%ecx\n 6: mov %%rcx,%%rdx")
#define JC1(N, C) JCC(C)
ALLCC(JC1)
INK(k_jrcxz, "mov $0,%%edx\n jrcxz 5f\n jmp 6f\n 5: mov $1,%%edx\n 6:") INK(k_jecxz, "mov $0,%%edx\n jecxz 5f\n jmp 6f\n 5: mov $1,%%edx\n 6:")

static Op *reg_mv(const char *name, KF k, int kind, int wb, int swb, int dst, int src, int imm, int memoff, int alias, int mem_r, int cc, uint32_t fmask) {
    Op *o = newop(name, k, kind >= MV_JCC ? m_jcc : m_intmove, 32, EK_I8);
    o->p = (void *)(intptr_t)(kind | (wb << 8) | (swb << 12) | (dst << 16) | (src << 20) | (cc << 24)); o->sz = wb ? wb : 8; o->imm = imm; o->memoff = memoff; o->alias = alias; o->mem_r = mem_r; o->nsrc = 2; o->exh = 0;
    xall(o, fmask); return o;
}
#define N_(...) (snprintf(b, sizeof b, __VA_ARGS__), nm_(b, ""))
static void reg_intmove_ops(void) {
    char b[80];
#define MOVR(W, WB, I3v) \
    reg_mv(N_("mov_r" #W "_r" #W), k_mov_##W##_rr, MV_MOV, WB, 0, 0, 1, 0, 0, AL_DD, 0, 0, FL_ALL); reg_mv(N_("mov_r" #W "_r" #W "_swap"), k_mov_##W##_rr2, MV_MOV, WB, 0, 1, 0, 0, 0, AL_DD, 0, 0, FL_ALL); \
    reg_mv(N_("mov_r" #W "_m" #W), k_mov_##W##_rm, MV_MOV, WB, 0, 0, 2, 0, 0, AL_MEM, 0, 0, FL_ALL); reg_mv(N_("mov_r" #W "_m" #W "_o3"), k_mov_##W##_rm3, MV_MOV, WB, 0, 1, 2, 0, 3, AL_MEM, 0, 0, FL_ALL); \
    reg_mv(N_("mov_m" #W "_r" #W), k_mov_##W##_mr0, MV_MOV, WB, 0, 2, 0, 0, 0, AL_DD, 1, 0, FL_ALL); reg_mv(N_("mov_m" #W "_r" #W "_o3"), k_mov_##W##_mr3, MV_MOV, WB, 0, 2, 1, 0, 3, AL_DD, 1, 0, FL_ALL); \
    reg_mv(N_("mov_r" #W "_imm"), k_mov_##W##_ri, MV_MOV, WB, 0, 0, 3, I3v, 0, AL_DD, 0, 0, FL_ALL); reg_mv(N_("mov_r" #W "_imm_m1"), k_mov_##W##_ri2, MV_MOV, WB, 0, 1, 3, -1, 0, AL_DD, 0, 0, FL_ALL); \
    reg_mv(N_("mov_m" #W "_imm"), k_mov_##W##_mi, MV_MOV, WB, 0, 2, 3, 0x55, 3, AL_DD, 1, 0, FL_ALL);
    MOVR(8, 1, -128) MOVR(16, 2, -32768) MOVR(32, 4, (int)0x80000000) MOVR(64, 8, (int)0x80000000)
    reg_mv("movabs_r64_imm64", k_movabs, MV_MOV, 8, 8, 0, 3, 0, 0, AL_DD, 0, 0, FL_ALL);
#define EXTR(NAME, KIND, WB, SWB) \
    reg_mv(N_(#NAME "_rr"), k_##NAME##_rr, KIND, WB, SWB, 0, 1, 0, 0, AL_DD, 0, 0, FL_ALL); reg_mv(N_(#NAME "_same"), k_##NAME##_aa, KIND, WB, SWB, 1, 1, 0, 0, AL_DD, 0, 0, FL_ALL); \
    reg_mv(N_(#NAME "_rm"), k_##NAME##_rm, KIND, WB, SWB, 0, 2, 0, 0, AL_MEM, 0, 0, FL_ALL); reg_mv(N_(#NAME "_rm_o3"), k_##NAME##_rm3, KIND, WB, SWB, 1, 2, 0, 3, AL_MEM, 0, 0, FL_ALL);
    EXTR(movzbw, MV_MOVZX, 2, 1) EXTR(movzbl, MV_MOVZX, 4, 1) EXTR(movzbq, MV_MOVZX, 8, 1) EXTR(movzwl, MV_MOVZX, 4, 2) EXTR(movzwq, MV_MOVZX, 8, 2)
    EXTR(movsbw, MV_MOVSX, 2, 1) EXTR(movsbl, MV_MOVSX, 4, 1) EXTR(movsbq, MV_MOVSX, 8, 1) EXTR(movswl, MV_MOVSX, 4, 2) EXTR(movswq, MV_MOVSX, 8, 2) EXTR(movslq, MV_MOVSX, 8, 4)
#define XCHR(W, WB) \
    reg_mv(N_("xchg_r" #W "_r" #W), k_xchg_##W##_rr, MV_XCHG, WB, 0, 0, 1, 0, 0, AL_DD, 0, 0, FL_ALL); reg_mv(N_("xchg_r" #W "_same"), k_xchg_##W##_aa, MV_XCHG, WB, 0, 0, 0, 0, 0, AL_DD, 0, 0, FL_ALL); \
    reg_mv(N_("xchg_m" #W "_r" #W), k_xchg_##W##_mr0, MV_XCHG, WB, 0, 2, 0, 0, 0, AL_DD, 1, 0, FL_ALL); reg_mv(N_("xchg_m" #W "_r" #W "_o3"), k_xchg_##W##_mr3, MV_XCHG, WB, 0, 2, 1, 0, 3, AL_DD, 1, 0, FL_ALL);
    XCHR(8, 1) XCHR(16, 2) XCHR(32, 4) XCHR(64, 8)
    static const char *cn[16] = {"o", "no", "b", "ae", "e", "ne", "be", "a", "s", "ns", "p", "np", "l", "ge", "le", "g"};
#define CCR(C, I) \
    reg_mv(N_("cmov" #C "_r16_r16"), k_cmov##C##_16_rr, MV_CMOV, 2, 0, 0, 1, 0, 0, AL_DD, 0, I, FL_ALL); reg_mv(N_("cmov" #C "_r32_r32"), k_cmov##C##_32_rr, MV_CMOV, 4, 0, 0, 1, 0, 0, AL_DD, 0, I, FL_ALL); \
    reg_mv(N_("cmov" #C "_r64_r64"), k_cmov##C##_64_rr, MV_CMOV, 8, 0, 0, 1, 0, 0, AL_DD, 0, I, FL_ALL); reg_mv(N_("cmov" #C "_r16_m16"), k_cmov##C##_16_rm, MV_CMOV, 2, 0, 0, 2, 0, 0, AL_MEM, 0, I, FL_ALL); \
    reg_mv(N_("cmov" #C "_r32_m32"), k_cmov##C##_32_rm, MV_CMOV, 4, 0, 0, 2, 0, 0, AL_MEM, 0, I, FL_ALL); reg_mv(N_("cmov" #C "_r64_m64"), k_cmov##C##_64_rm, MV_CMOV, 8, 0, 0, 2, 0, 0, AL_MEM, 0, I, FL_ALL); \
    reg_mv(N_("cmov" #C "_r32_same"), k_cmov##C##_32_aa, MV_CMOV, 4, 0, 0, 0, 0, 0, AL_DD, 0, I, FL_ALL); reg_mv(N_("cmov" #C "_r64_m64_o3"), k_cmov##C##_64_rm3, MV_CMOV, 8, 0, 1, 2, 0, 3, AL_MEM, 0, I, FL_ALL); \
    reg_mv(N_("set" #C "_r8"), k_set##C##_r, MV_SETCC, 1, 0, 0, 3, 0, 0, AL_DD, 0, I, FL_ALL); reg_mv(N_("set" #C "_r8_b"), k_set##C##_r2, MV_SETCC, 1, 0, 1, 3, 0, 0, AL_DD, 0, I, FL_ALL); \
    reg_mv(N_("set" #C "_m8"), k_set##C##_m, MV_SETCC, 1, 0, 2, 3, 0, 3, AL_DD, 1, I, FL_ALL); \
    reg_mv(N_("j" #C "_rel8"), k_j##C##_8, MV_JCC, 0, 0, 0, 0, 0, 0, AL_DD, 0, I, FL_ALL); reg_mv(N_("j" #C "_rel32"), k_j##C##_32, MV_JCC, 0, 0, 0, 0, 0, 0, AL_DD, 0, I, FL_ALL);
    (void)cn;
    CCR(o, 0) CCR(no, 1) CCR(b, 2) CCR(ae, 3) CCR(e, 4) CCR(ne, 5) CCR(be, 6) CCR(a, 7) CCR(s, 8) CCR(ns, 9) CCR(p, 10) CCR(np, 11) CCR(l, 12) CCR(ge, 13) CCR(le, 14) CCR(g, 15)
    reg_mv("jrcxz", k_jrcxz, MV_JRCXZ, 0, 0, 0, 0, 0, 0, AL_DD, 0, 0, FL_ALL); reg_mv("jecxz", k_jecxz, MV_JECXZ, 0, 0, 0, 0, 0, 0, AL_DD, 0, 0, FL_ALL);
#define LEAR(NAME, DISP, SC, BASE, A32) \
    reg_mv(N_("lea_" #NAME "_r64"), k_lea_##NAME##_64, MV_LEA, 8, SC, 3, 0, DISP, 0, AL_DD, 0, BASE | (A32 << 1), FL_ALL); reg_mv(N_("lea_" #NAME "_r32"), k_lea_##NAME##_32, MV_LEA, 4, SC, 3, 0, DISP, 0, AL_DD, 0, BASE | (A32 << 1), FL_ALL); \
    reg_mv(N_("lea_" #NAME "_r16"), k_lea_##NAME##_16, MV_LEA, 2, SC, 3, 0, DISP, 0, AL_DD, 0, BASE | (A32 << 1), FL_ALL);
    LEAR(b0, 0, 0, 1, 0) LEAR(b7f, 0x7f, 0, 1, 0) LEAR(bd32, 0x12345678, 0, 1, 0) LEAR(bneg, -8, 0, 1, 0) LEAR(bi1, 0, 1, 1, 0) LEAR(bi2, 0, 2, 1, 0) LEAR(bi4, -0x10, 4, 1, 0) LEAR(bi8, 0x12345678, 8, 1, 0)
    LEAR(i2, 0x40, 2, 0, 0) LEAR(i4, 0, 4, 0, 0) LEAR(i8, -3, 8, 0, 0) LEAR(abs, 0x1234, 1, 0, 0) LEAR(a32b, 0x7f, 0, 1, 1) LEAR(a32bi, 0, 4, 1, 1) LEAR(a32bi8, -0x1000, 8, 1, 1)
}
#endif
