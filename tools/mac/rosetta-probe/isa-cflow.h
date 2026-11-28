// Family x86_control_flow: push/pop (register, 16-bit, immediate, memory), call/ret (relative, through register, through memory, ret imm16), jmp (rel8, rel32, register,
// memory), leave, the nop encodings, fences, pause, clc/stc/cmc, lahf/sahf. The stack pointer deltas, the return address, the value that comes back, and the flags (which none of
// these changes, except the flag instructions) are all checked. ud2 and int3 raise exceptions and are the subject of the exception tests, not of this probe.
#ifndef ISA_CFLOW_H
#define ISA_CFLOW_H
#include "isa-int.h"
enum { CF_PUSHPOP, CF_PUSHIMM8, CF_PUSHIMM32A, CF_PUSHIMM32B, CF_PUSHM, CF_POPM, CF_PUSHPOP16, CF_PUSHRSP, CF_CALL, CF_JMP, CF_RETIMM, CF_LEAVE, CF_NOP, CF_CLC, CF_STC, CF_CMC, CF_LAHF, CF_SAHF };
#define CK(NAME, BODY) static void NAME(const IRec *in, Out *out, long n) { \
    A("1:\n vmovups 0(%0),%%ymm0\n vmovups %%ymm0,(%1)\n mov 0(%0),%%rdx\n mov 32(%0),%%rax\n mov 64(%0),%%rbx\n mov 96(%0),%%rcx\n pushq 104(%0)\n popfq\n " BODY \
      "\n pushfq\n popq 40(%1)\n mov %%rax,32(%1)\n mov %%rdx,48(%1)\n mov %%rbx,56(%1)\n add $128,%0\n add $64,%1\n dec %2\n jnz 1b" \
      : "+r"(in), "+r"(out), "+r"(n) :: "rax", "rbx", "rcx", "rdx", "r8", "r9", "r10", "ymm0", "memory", "cc"); }
#define DELTA(X, Y) "pushfq\n sub %%" Y ",%%" X "\n popfq\n"
CK(k_pp, "mov %%rsp,%%r8\n push %%rax\n mov %%rsp,%%r9\n pop %%rdx\n " DELTA("r8", "r9") " mov %%r8,%%rbx")
CK(k_pimm8, "push $-5\n pop %%rdx") CK(k_pimm8b, "push $0x7f\n pop %%rdx") CK(k_pimm32a, "push $0x12345678\n pop %%rdx") CK(k_pimm32b, "push $-0x12345678\n pop %%rdx")
CK(k_pushm, "push 64(%0)\n pop %%rdx") CK(k_pushm3, "push 67(%0)\n pop %%rdx") CK(k_popm, "push %%rax\n pop (%1)") CK(k_popm3, "push %%rax\n pop 3(%1)")
CK(k_pp16, "mov %%rsp,%%r8\n push %%ax\n mov %%rsp,%%r9\n pop %%dx\n " DELTA("r8", "r9") " mov %%r8,%%rbx")
CK(k_pushrsp, "mov %%rsp,%%rcx\n push %%rsp\n pop %%rdx\n " DELTA("rdx", "rcx"))
#define CALLB(PRE, CALL) PRE " lea 7f(%%rip),%%r9\n " CALL "\n 7: mov $2,%%ebx\n jmp 6f\n 5: mov (%%rsp),%%r8\n " DELTA("r8", "r9") " lea 1(%%r8),%%rdx\n ret\n 6:"
CK(k_call_rel, "mov $0,%%edx\n lea 7f(%%rip),%%r9\n call 5f\n 7: mov $2,%%ebx\n jmp 6f\n 5: mov (%%rsp),%%r8\n " DELTA("r8", "r9") " lea 1(%%r8),%%rdx\n ret\n 6:")
CK(k_call_reg, "lea 5f(%%rip),%%rcx\n lea 7f(%%rip),%%r9\n call *%%rcx\n 7: mov $2,%%ebx\n jmp 6f\n 5: mov (%%rsp),%%r8\n " DELTA("r8", "r9") " lea 1(%%r8),%%rdx\n ret\n 6:")
CK(k_call_mem, "lea 5f(%%rip),%%rcx\n push %%rcx\n lea 7f(%%rip),%%r9\n call *(%%rsp)\n 7: lea 8(%%rsp),%%rsp\n mov $2,%%ebx\n jmp 6f\n 5: mov (%%rsp),%%r8\n " DELTA("r8", "r9") " lea 1(%%r8),%%rdx\n ret\n 6:")
CK(k_jmp8, "mov $0,%%edx\n jmp 5f\n mov $9,%%edx\n 5: lea 1(%%rdx),%%rdx")
CK(k_jmp32, "mov $0,%%edx\n jmp 5f\n .fill 200,1,0x90\n mov $9,%%edx\n 5: lea 1(%%rdx),%%rdx")
CK(k_jmp_reg, "mov $0,%%edx\n lea 5f(%%rip),%%rcx\n jmp *%%rcx\n mov $9,%%edx\n 5: lea 1(%%rdx),%%rdx")
CK(k_jmp_mem, "mov $0,%%edx\n lea 5f(%%rip),%%rcx\n push %%rcx\n jmp *(%%rsp)\n mov $9,%%edx\n 5: lea 8(%%rsp),%%rsp\n lea 1(%%rdx),%%rdx")
CK(k_retimm, "mov %%rsp,%%r8\n push %%rax\n call 5f\n jmp 6f\n 5: ret $8\n 6: mov %%rsp,%%r9\n " DELTA("r8", "r9") " lea 1(%%r8),%%rdx")
CK(k_leave, "mov %%rbp,%%r10\n mov %%rax,%%rbp\n mov %%rsp,%%r8\n push %%rbp\n mov %%rsp,%%rbp\n lea -48(%%rsp),%%rsp\n leave\n mov %%rsp,%%r9\n " DELTA("r8", "r9") " mov %%rbp,%%rbx\n mov %%r10,%%rbp\n lea 1(%%r8),%%rdx")
CK(k_nop1, "nop") CK(k_nop2, "xchg %%ax,%%ax") CK(k_nop3, "nopl 0x0(%%rax)") CK(k_nop4, "nopw 0x0(%%rax,%%rax,1)") CK(k_nop5, ".byte 0x66,0x2e,0x0f,0x1f,0x84,0,0,0,0,0") CK(k_nop6, ".byte 0x0f,0x1f,0x80,0,0,0,0")
CK(k_nop7, ".byte 0x66,0x66,0x2e,0x0f,0x1f,0x84,0,0,0,0,0") CK(k_nop8, "nopl (%%rbx)") CK(k_nop9, "nopw 0x7f(%%rax,%%rbx,8)") CK(k_pause, "pause") CK(k_lfence, "lfence") CK(k_mfence, "mfence") CK(k_sfence, "sfence")
CK(k_clc, "clc") CK(k_stc, "stc") CK(k_cmc, "cmc") CK(k_lahf, "lahf") CK(k_sahf, "sahf")
// op->p: kind
static void m_cf(const Ctx *c, const IRec *r, Out *o) {
    int k = (int)(intptr_t)c->op->p; uint64_t rax = ld_(r->a, 8), rbx = ld_(r->b, 8), rdx = ld_(r->d, 8); uint32_t f = (uint32_t)ld_(r->c + 8, 8) & FL_ALL;
    switch (k) {
    case CF_PUSHPOP: rdx = rax; rbx = 8; break;
    case CF_PUSHIMM8: rdx = c->op->imm < 0 ? (uint64_t)(int64_t)c->op->imm : (uint64_t)c->op->imm; break;
    case CF_PUSHIMM32A: case CF_PUSHIMM32B: rdx = (uint64_t)(int64_t)c->op->imm; break;
    case CF_PUSHM: rdx = ld_(msrc(r, c->op), 8); break;
    case CF_POPM: st_(o->r + c->op->memoff, 8, rax); break;
    case CF_PUSHPOP16: rdx = (rdx & ~0xFFFFull) | (rax & 0xFFFF); rbx = 2; break;
    case CF_PUSHRSP: rdx = 0; break;
    case CF_CALL: rdx = 1; rbx = 2; break;
    case CF_JMP: rdx = 1; break;
    case CF_RETIMM: case CF_LEAVE: rdx = 1; if (k == CF_LEAVE) rbx = rax; break;
    case CF_NOP: break;
    case CF_CLC: f &= ~FL_CF; break; case CF_STC: f |= FL_CF; break; case CF_CMC: f ^= FL_CF; break;
    case CF_LAHF: rax = (rax & ~0xFF00ull) | ((uint64_t)((f & FL_SF ? 0x80 : 0) | (f & FL_ZF ? 0x40 : 0) | (f & FL_AF ? 0x10 : 0) | (f & FL_PF ? 4 : 0) | 2 | (f & FL_CF ? 1 : 0)) << 8); break;
    case CF_SAHF: { uint32_t ah = (uint32_t)(rax >> 8) & 0xFF; f = (f & ~(uint32_t)(FL_SF | FL_ZF | FL_AF | FL_PF | FL_CF)) | ((ah & 0x80) ? FL_SF : 0) | ((ah & 0x40) ? FL_ZF : 0) | ((ah & 0x10) ? FL_AF : 0) | ((ah & 4) ? FL_PF : 0) | ((ah & 1) ? FL_CF : 0); break; }
    }
    o->x[0] = rax; o->x[1] = f; o->x[2] = rdx; o->x[3] = rbx;
}
static void reg_cflow_ops(void) {
#define RC(NAME, K, KIND, IMM, MEMOFF, ALIAS, MEMR) { Op *o = newop(NAME, K, m_cf, 32, EK_I8); o->p = (void *)(intptr_t)(KIND); o->sz = 8; o->imm = IMM; o->memoff = MEMOFF; o->alias = ALIAS; o->mem_r = MEMR; o->nsrc = 2; xall(o, FL_ALL); o->fix = fix_int; }
    RC("push_pop_r64", k_pp, CF_PUSHPOP, 0, 0, AL_DD, 0) RC("push_imm8_neg", k_pimm8, CF_PUSHIMM8, -5, 0, AL_DD, 0) RC("push_imm8_pos", k_pimm8b, CF_PUSHIMM8, 0x7f, 0, AL_DD, 0)
    RC("push_imm32_pos", k_pimm32a, CF_PUSHIMM32A, 0x12345678, 0, AL_DD, 0) RC("push_imm32_neg", k_pimm32b, CF_PUSHIMM32B, -0x12345678, 0, AL_DD, 0)
    RC("push_m64", k_pushm, CF_PUSHM, 0, 0, AL_MEM, 0) RC("push_m64_o3", k_pushm3, CF_PUSHM, 0, 3, AL_MEM, 0) RC("pop_m64", k_popm, CF_POPM, 0, 0, AL_DD, 1) RC("pop_m64_o3", k_popm3, CF_POPM, 0, 3, AL_DD, 1)
    RC("push_pop_r16", k_pp16, CF_PUSHPOP16, 0, 0, AL_DD, 0) RC("push_rsp", k_pushrsp, CF_PUSHRSP, 0, 0, AL_DD, 0)
    RC("call_rel32_ret", k_call_rel, CF_CALL, 0, 0, AL_DD, 0) RC("call_reg_ret", k_call_reg, CF_CALL, 0, 0, AL_DD, 0) RC("call_mem_ret", k_call_mem, CF_CALL, 0, 0, AL_DD, 0)
    RC("jmp_rel8", k_jmp8, CF_JMP, 0, 0, AL_DD, 0) RC("jmp_rel32", k_jmp32, CF_JMP, 0, 0, AL_DD, 0) RC("jmp_reg", k_jmp_reg, CF_JMP, 0, 0, AL_DD, 0) RC("jmp_mem", k_jmp_mem, CF_JMP, 0, 0, AL_DD, 0)
    RC("ret_imm16", k_retimm, CF_RETIMM, 0, 0, AL_DD, 0) RC("leave", k_leave, CF_LEAVE, 0, 0, AL_DD, 0)
    RC("nop", k_nop1, CF_NOP, 0, 0, AL_DD, 0) RC("nop_xchg_ax_ax", k_nop2, CF_NOP, 0, 0, AL_DD, 0) RC("nop_m32", k_nop3, CF_NOP, 0, 0, AL_DD, 0) RC("nop_m16_sib", k_nop4, CF_NOP, 0, 0, AL_DD, 0)
    RC("nop_cs_m16_disp32", k_nop5, CF_NOP, 0, 0, AL_DD, 0) RC("nop_m32_disp32", k_nop6, CF_NOP, 0, 0, AL_DD, 0) RC("nop_data16x2_cs", k_nop7, CF_NOP, 0, 0, AL_DD, 0) RC("nop_m32_rbx", k_nop8, CF_NOP, 0, 0, AL_DD, 0) RC("nop_m16_sib_disp8", k_nop9, CF_NOP, 0, 0, AL_DD, 0)
    RC("pause", k_pause, CF_NOP, 0, 0, AL_DD, 0) RC("lfence", k_lfence, CF_NOP, 0, 0, AL_DD, 0) RC("mfence", k_mfence, CF_NOP, 0, 0, AL_DD, 0) RC("sfence", k_sfence, CF_NOP, 0, 0, AL_DD, 0)
    RC("clc", k_clc, CF_CLC, 0, 0, AL_DD, 0) RC("stc", k_stc, CF_STC, 0, 0, AL_DD, 0) RC("cmc", k_cmc, CF_CMC, 0, 0, AL_DD, 0) RC("lahf", k_lahf, CF_LAHF, 0, 0, AL_DD, 0) RC("sahf", k_sahf, CF_SAHF, 0, 0, AL_DD, 0)
}
static int plan_cf(const Op *o) {
    int fl = 0; long n = 1L << 20; fl += run_stream_op(o, 0x1F80, n, 0, NULL);
    if ((o->alias == AL_MEM || o->mem_r) && o->al == 0) fl += run_straddle(o);
    return fl;
}
#endif
