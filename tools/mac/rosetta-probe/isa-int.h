// Family x86_int_alu: add sub adc sbb and or xor cmp test (reg,reg / reg,reg alias / reg,mem / mem,reg at two offsets / reg,imm on rax and rbx, widths 8/16/32/64),
// and inc dec neg not (register and memory, all widths), with the six arithmetic flags compared bit by bit (undefined flags excluded per SDM).
// Integer kernels: rdx = d[0..7], rax = a[0..7], rbx = b[0..7], rcx = c[0..7], initial flags = c[8..15]; results: x[0] = rax, x[1] = flags, x[2] = rdx, x[3] = rbx; memory
// destinations live in out.r (pre-filled with d) at op->memoff.
#ifndef ISA_INT_H
#define ISA_INT_H
#include "isa-model-int.h"
#define INK(NAME, BODY) static void NAME(const IRec *in, Out *out, long n) { \
    A("1:\n vmovups 0(%0),%%ymm0\n vmovups %%ymm0,(%1)\n mov 0(%0),%%rdx\n mov 32(%0),%%rax\n mov 64(%0),%%rbx\n mov 96(%0),%%rcx\n pushq 104(%0)\n popfq\n " BODY \
      "\n pushfq\n popq 40(%1)\n mov %%rax,32(%1)\n mov %%rdx,48(%1)\n mov %%rbx,56(%1)\n add $128,%0\n add $64,%1\n dec %2\n jnz 1b" \
      : "+r"(in), "+r"(out), "+r"(n) :: "rax", "rbx", "rcx", "rdx", "ymm0", "memory", "cc"); }
// generator hook: initial flags and width-specific edge values (the exhaustive 8-bit streams keep their operands, only the flags vary; exh == 2 sweeps CF)
static void fix_int(IRec *r, const Op *o, int stream) {
    int w = o->sz * 8; uint64_t m = mskb(w), a = ld_(r->a, 8), b = ld_(r->b, 8); uint64_t fl = rnd();
    if (stream != 6) {
        switch (rnd() % 7) {
        case 0: a = (a & ~m) | gi_el(w, 1); break; case 1: b = (b & ~m) | gi_el(w, 1); break;
        case 2: a = (a & ~m) | gi_el(w, 1); b = (b & ~m) | gi_el(w, 1); break;
        case 3: b = (b & ~m) | ((a + (rnd() % 5) - 2) & m); break; case 4: b = (b & ~m) | (a & m); break;
        case 5: a = (a & ~m) | ((b + (rnd() % 3) - 1) & m); break; default: break;
        }
        st_(r->a, 8, a); st_(r->b, 8, b);
        if (o->mem_r && (rnd() & 1)) st_(r->d + o->memoff, o->sz, (rnd() & 1) ? gi_el(w, 1) : ((a + (rnd() % 3) - 1) & m));          // memory destination: edge value or near the register operand
        if (o->alias == AL_MEM && o->memoff && (rnd() & 1)) st_(r->b + o->memoff, o->sz, (rnd() & 1) ? gi_el(w, 1) : ((a + (rnd() % 3) - 1) & m));   // memory source at an odd offset
    } else if (o->exh == 2) fl = (fl & ~1ull) | (uint64_t)(g_exh_hi & 1);
    st_(r->c + 8, 8, (fl & FL_ALL) | 2);
}
#define ALU2(M, W, RA, RB, I3) \
    INK(k_##M##_##W##_rr, #M " %%" RB ",%%" RA) INK(k_##M##_##W##_rr2, #M " %%" RA ",%%" RB) INK(k_##M##_##W##_aa, #M " %%" RA ",%%" RA) \
    INK(k_##M##_##W##_rm, #M " 64(%0),%%" RA) INK(k_##M##_##W##_rm3, #M " 67(%0),%%" RB) INK(k_##M##_##W##_mr0, #M " %%" RA ",(%1)") INK(k_##M##_##W##_mr3, #M " %%" RB ",3(%1)") \
    INK(k_##M##_##W##_ri1, #M " $-1,%%" RA) INK(k_##M##_##W##_ri2, #M " $0x7f,%%" RB) INK(k_##M##_##W##_ri3, #M " $" I3 ",%%" RB)
#define ALU2ALL(M) ALU2(M, 8, "al", "bl", "0x80") ALU2(M, 16, "ax", "bx", "0x8000") ALU2(M, 32, "eax", "ebx", "0x80000000") ALU2(M, 64, "rax", "rbx", "-2147483648")
ALU2ALL(add) ALU2ALL(sub) ALU2ALL(adc) ALU2ALL(sbb) ALU2ALL(and) ALU2ALL(or) ALU2ALL(xor) ALU2ALL(cmp) ALU2ALL(test)
#define ALU1(M, W, RA, RB) INK(k_##M##_##W##_r, #M " %%" RA) INK(k_##M##_##W##_r2, #M " %%" RB) INK(k_##M##_##W##_m0, #M " (%1)") INK(k_##M##_##W##_m3, #M " 3(%1)")
// inc/dec/neg/not need an operand size on memory forms
#define ALU1S(M, W, RA, RB, SFX) INK(k_##M##_##W##_r, #M " %%" RA) INK(k_##M##_##W##_r2, #M " %%" RB) INK(k_##M##_##W##_m0, #M SFX " (%1)") INK(k_##M##_##W##_m3, #M SFX " 3(%1)")
#define ALU1ALL(M) ALU1S(M, 8, "al", "bl", "b") ALU1S(M, 16, "ax", "bx", "w") ALU1S(M, 32, "eax", "ebx", "l") ALU1S(M, 64, "rax", "rbx", "q")
ALU1ALL(inc) ALU1ALL(dec) ALU1ALL(neg) ALU1ALL(not)

static void xall(Op *o, uint32_t fmask) { o->cr = o->mem_r ? 32 : 0; o->xm[0] = o->xm[2] = o->xm[3] = ~0ull; o->xm[1] = fmask; o->fix = fix_int; }
static Op *reg_int(const char *name, KF k, int kind, int wb, int dst, int src, int imm, int memoff, int alias, int mem_r, uint32_t fmask, int exh) {
    Op *o = newop(name, k, m_alu, 32, EK_I8); o->p = (void *)(intptr_t)(kind | (wb << 8) | (dst << 16) | (src << 20)); o->sz = wb; o->imm = imm; o->memoff = memoff; o->alias = alias; o->mem_r = mem_r; o->nsrc = 2;
    o->exh = exh; xall(o, fmask); return o;
}
#define R2(M, KIND, W, WB, FM, EXH, I3) { char b[64]; \
    snprintf(b, sizeof b, #M "_r" #W "_r" #W); reg_int(nm_(b, ""), k_##M##_##W##_rr, KIND, WB, 0, 1, 0, 0, AL_DD, 0, FM, EXH); \
    snprintf(b, sizeof b, #M "_r" #W "_r" #W "_swap"); reg_int(nm_(b, ""), k_##M##_##W##_rr2, KIND, WB, 1, 0, 0, 0, AL_DD, 0, FM, EXH ? 1 : 0); \
    snprintf(b, sizeof b, #M "_r" #W "_same"); reg_int(nm_(b, ""), k_##M##_##W##_aa, KIND, WB, 0, 0, 0, 0, AL_DD, 0, FM, 0); \
    snprintf(b, sizeof b, #M "_r" #W "_m" #W); reg_int(nm_(b, ""), k_##M##_##W##_rm, KIND, WB, 0, 2, 0, 0, AL_MEM, 0, FM, 0); \
    snprintf(b, sizeof b, #M "_r" #W "_m" #W "_o3"); reg_int(nm_(b, ""), k_##M##_##W##_rm3, KIND, WB, 1, 2, 0, 3, AL_MEM, 0, FM, 0); \
    snprintf(b, sizeof b, #M "_m" #W "_r" #W); reg_int(nm_(b, ""), k_##M##_##W##_mr0, KIND, WB, 2, 0, 0, 0, AL_DD, 1, FM, 0); \
    snprintf(b, sizeof b, #M "_m" #W "_r" #W "_o3"); reg_int(nm_(b, ""), k_##M##_##W##_mr3, KIND, WB, 2, 1, 0, 3, AL_DD, 1, FM, 0); \
    snprintf(b, sizeof b, #M "_r" #W "_imm_m1"); reg_int(nm_(b, ""), k_##M##_##W##_ri1, KIND, WB, 0, 3, -1, 0, AL_DD, 0, FM, 0); \
    snprintf(b, sizeof b, #M "_r" #W "_imm_7f"); reg_int(nm_(b, ""), k_##M##_##W##_ri2, KIND, WB, 1, 3, 0x7f, 0, AL_DD, 0, FM, 0); \
    snprintf(b, sizeof b, #M "_r" #W "_imm_hi"); reg_int(nm_(b, ""), k_##M##_##W##_ri3, KIND, WB, 1, 3, (WB == 1 ? -128 : WB == 2 ? -32768 : (int)0x80000000), 0, AL_DD, 0, FM, 0); }
#define R2ALL(M, KIND, FM, EXH) R2(M, KIND, 8, 1, FM, EXH, 0) R2(M, KIND, 16, 2, FM, 0, 0) R2(M, KIND, 32, 4, FM, 0, 0) R2(M, KIND, 64, 8, FM, 0, 0)
#define R1(M, KIND, W, WB, FM) { char b[64]; \
    snprintf(b, sizeof b, #M "_r" #W); reg_int(nm_(b, ""), k_##M##_##W##_r, KIND, WB, 0, 4, 0, 0, AL_DD, 0, FM, W == 8 ? 1 : 0); \
    snprintf(b, sizeof b, #M "_r" #W "_b"); reg_int(nm_(b, ""), k_##M##_##W##_r2, KIND, WB, 1, 4, 0, 0, AL_DD, 0, FM, 0); \
    snprintf(b, sizeof b, #M "_m" #W); reg_int(nm_(b, ""), k_##M##_##W##_m0, KIND, WB, 2, 4, 0, 0, AL_DD, 1, FM, 0); \
    snprintf(b, sizeof b, #M "_m" #W "_o3"); reg_int(nm_(b, ""), k_##M##_##W##_m3, KIND, WB, 2, 4, 0, 3, AL_DD, 1, FM, 0); }
#define R1ALL(M, KIND, FM) R1(M, KIND, 8, 1, FM) R1(M, KIND, 16, 2, FM) R1(M, KIND, 32, 4, FM) R1(M, KIND, 64, 8, FM)
static void reg_alu_ops(void) {
    R2ALL(add, IA_ADD, FL_ALL, 1) R2ALL(sub, IA_SUB, FL_ALL, 1) R2ALL(adc, IA_ADC, FL_ALL, 2) R2ALL(sbb, IA_SBB, FL_ALL, 2)
    R2ALL(and, IA_AND, FL_ALL & ~FL_AF, 1) R2ALL(or, IA_OR, FL_ALL & ~FL_AF, 1) R2ALL(xor, IA_XOR, FL_ALL & ~FL_AF, 1) R2ALL(cmp, IA_CMP, FL_ALL, 1) R2ALL(test, IA_TEST, FL_ALL & ~FL_AF, 1)
    R1ALL(inc, IA_INC, FL_ALL) R1ALL(dec, IA_DEC, FL_ALL) R1ALL(neg, IA_NEG, FL_ALL) R1ALL(not, IA_NOT, FL_ALL)
}
static int plan_int(const Op *o) {
    int fl = 0; long n = o->mem_r || o->alias == AL_MEM ? (2L << 20) : (4L << 20);
    fl += run_stream_op(o, 0x1F80, n, o->exh, NULL);
    if ((o->alias == AL_MEM || o->mem_r) && o->al == 0) fl += run_straddle(o);
    return fl;
}
#endif
