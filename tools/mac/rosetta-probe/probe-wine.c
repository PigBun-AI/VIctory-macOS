// Rosetta-under-Wine probe: a Windows x86-64 exe (cross-built with llvm-mingw) that runs one case in one mode
// and prints one "RES ..." line. Page faults are taken by a vectored exception handler (VEH) that opens the
// page with VirtualProtect and returns EXCEPTION_CONTINUE_EXECUTION - what the runtime's guards do.
// Driver: ../rosetta-probe-wine.sh (one process per case and mode, because a fatal Rosetta error ends the process).
//
//   probe-wine.exe list | cpuid | page4k | batch <name> | <case> <mode> [back=N] [b=0xOFF] [gran=N] [mask=NAME] [imm=N]
//   modes: inpage  fault  ro  cross  crossro  crossfirst  crossfirstro  split (crosses the boundary, nothing protected)
//     fault/ro        the page holding the destination is PAGE_NOACCESS / PAGE_READONLY
//     cross/crossro   destination starts `back` bytes before a boundary, the page after it is protected
//     crossfirst(ro)  the page before the boundary is protected, the access continues into an open page
//   mask names (masked stores): full lo hi l0 last zero alt
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <cpuid.h>
#include "softref.h"

typedef unsigned char U8;
#define ARITH 0x8D5u                       // CF PF AF ZF SF OF
static U8 *g_base;
static const size_t g_size = 0x20000;
static void *volatile g_ip;                // address of the instruction under test (set by each asm block)
typedef struct { DWORD code; void *exaddr; void *rip; ULONG_PTR type, va; } Rec;
static Rec g_rec[32];
static volatile LONG g_nrec, g_nbad_va, g_nother;
static int g_skip, g_skip_always;                         // >0: bytes to step over when an unexpected exception hits (clwb SIGILL ...)

static LONG CALLBACK veh(PEXCEPTION_POINTERS ep) {
    EXCEPTION_RECORD *er = ep->ExceptionRecord; DWORD old;
    LONG i = InterlockedIncrement(&g_nrec) - 1;
    if (i >= 12) { printf("FAULT STORM at %p\n", er->ExceptionAddress); fflush(stdout); ExitProcess(9); }
    Rec *r = &g_rec[i]; r->code = er->ExceptionCode; r->exaddr = er->ExceptionAddress; r->rip = (void *)ep->ContextRecord->Rip;
    r->type = er->NumberParameters > 0 ? er->ExceptionInformation[0] : 99; r->va = er->NumberParameters > 1 ? er->ExceptionInformation[1] : 0;
    if (er->ExceptionCode == EXCEPTION_ACCESS_VIOLATION) {
        uintptr_t va = (uintptr_t)r->va, b = (uintptr_t)g_base;
        if (g_skip && g_skip_always) { ep->ContextRecord->Rip += g_skip; return EXCEPTION_CONTINUE_EXECUTION; }
        if (va >= b && va < b + g_size) { VirtualProtect((void *)(va & ~(uintptr_t)0xFFF), 0x1000, PAGE_READWRITE, &old); return EXCEPTION_CONTINUE_EXECUTION; }
        InterlockedIncrement(&g_nbad_va);
        if (g_skip) { ep->ContextRecord->Rip += g_skip; return EXCEPTION_CONTINUE_EXECUTION; }
        VirtualProtect(g_base, g_size, PAGE_READWRITE, &old);       // unknown address: open everything, keep going
        return EXCEPTION_CONTINUE_EXECUTION;
    }
    InterlockedIncrement(&g_nother);
    if (g_skip) { ep->ContextRecord->Rip += g_skip; return EXCEPTION_CONTINUE_EXECUTION; }
    return EXCEPTION_CONTINUE_SEARCH;
}

static uint64_t rs = 0x9E3779B97F4A7C15ull;
static uint64_t rnd(void) { rs ^= rs << 13; rs ^= rs >> 7; rs ^= rs << 17; return rs; }
static void say(const char *f, ...) { va_list a; va_start(a, f); vprintf(f, a); va_end(a); fflush(stdout); }

// ---------- flags models ----------
static uint32_t pf8(uint64_t r) { return (__builtin_popcount((unsigned)(r & 0xFF)) & 1) ? 0 : 4; }
static uint64_t msk(int bits) { return bits == 64 ? ~0ull : ((1ull << bits) - 1); }
static uint32_t fl_sub(uint64_t a, uint64_t b, int bits) {      // flags of CMP a,b
    uint64_t m = msk(bits), r = (a - b) & m; a &= m; b &= m;
    uint32_t f = 0; if (a < b) f |= 1; f |= pf8(r); if ((a ^ b ^ r) & 0x10) f |= 0x10; if (!r) f |= 0x40; if (r >> (bits - 1)) f |= 0x80;
    if (((a ^ b) & (a ^ r)) >> (bits - 1) & 1) f |= 0x800; return f;
}
static uint32_t fl_add(uint64_t a, uint64_t b, int bits) {      // flags of ADD a,b
    uint64_t m = msk(bits), r = (a + b) & m; a &= m; b &= m;
    uint32_t f = 0; if (r < a) f |= 1; f |= pf8(r); if ((a ^ b ^ r) & 0x10) f |= 0x10; if (!r) f |= 0x40; if (r >> (bits - 1)) f |= 0x80;
    if ((~(a ^ b) & (a ^ r)) >> (bits - 1) & 1) f |= 0x800; return f;
}

// ---------- store cases ----------
static U8 g_src[64], g_mask[32];
#define IP "leaq 1f(%%rip),%%r11\n movq %%r11,%[gip]\n"
#define GIP [gip] "=m"(g_ip)
#define A __asm__ volatile
typedef void (*RunFn)(U8 *p, const U8 *s, const U8 *m);
typedef struct { const char *name; int acc; int len, lane, mlanes, align, back, fsrc, skip; RunFn run; int model; } Case;
// acc: 0 no fault ever (prefetch), 1 read-type (clflush), 2 write. mlanes: >0 masked store with that many lanes. model: 0 lane copy, 1 none (memory unchanged),
// 2 vextract high half, 3 vcvtps2ph ymm imm, 4 vcvtps2ph xmm imm
static int g_imm;

static void r_mov32(U8 *p, const U8 *s, const U8 *m) { A("movl (%[s]),%%eax\n" IP "1: movl %%eax,(%[p])" : GIP : [p] "r"(p), [s] "r"(s) : "rax", "r11", "memory"); }
static void r_mov64(U8 *p, const U8 *s, const U8 *m) { A("movq (%[s]),%%rax\n" IP "1: movq %%rax,(%[p])" : GIP : [p] "r"(p), [s] "r"(s) : "rax", "r11", "memory"); }
#define STORE(N, LD, INSN, R) static void N(U8 *p, const U8 *s, const U8 *m) { A(LD " (%[s]),%%" R "0\n" IP "1: " INSN " %%" R "0,(%[p])" : GIP : [p] "r"(p), [s] "r"(s) : "r11", R "0", "memory"); }
STORE(r_vmovdqu_ymm, "vmovdqu", "vmovdqu", "ymm")
STORE(r_vmovups_ymm, "vmovups", "vmovups", "ymm")
STORE(r_vmovdqa_ymm, "vmovdqu", "vmovdqa", "ymm")
STORE(r_vmovaps_ymm, "vmovups", "vmovaps", "ymm")
STORE(r_vmovdqu_xmm, "vmovdqu", "vmovdqu", "xmm")
STORE(r_vmovups_xmm, "vmovups", "vmovups", "xmm")
STORE(r_movups_xmm, "movups", "movups", "xmm")
STORE(r_vmovntdq_ymm, "vmovdqu", "vmovntdq", "ymm")
STORE(r_vmovntps_ymm, "vmovups", "vmovntps", "ymm")
STORE(r_vmovntdq_xmm, "vmovdqu", "vmovntdq", "xmm")
STORE(r_movntdq_xmm, "movdqu", "movntdq", "xmm")
STORE(r_movntps_xmm, "movups", "movntps", "xmm")
static void r_movnti32(U8 *p, const U8 *s, const U8 *m) { A("movl (%[s]),%%eax\n" IP "1: movnti %%eax,(%[p])\n sfence" : GIP : [p] "r"(p), [s] "r"(s) : "rax", "r11", "memory"); }
static void r_movnti64(U8 *p, const U8 *s, const U8 *m) { A("movq (%[s]),%%rax\n" IP "1: movnti %%rax,(%[p])\n sfence" : GIP : [p] "r"(p), [s] "r"(s) : "rax", "r11", "memory"); }
static void r_vextractf128(U8 *p, const U8 *s, const U8 *m) { A("vmovups (%[s]),%%ymm0\n" IP "1: vextractf128 $1,%%ymm0,(%[p])" : GIP : [p] "r"(p), [s] "r"(s) : "r11", "ymm0", "memory"); }
static void r_vextracti128(U8 *p, const U8 *s, const U8 *m) { A("vmovups (%[s]),%%ymm0\n" IP "1: vextracti128 $1,%%ymm0,(%[p])" : GIP : [p] "r"(p), [s] "r"(s) : "r11", "ymm0", "memory"); }
#define MASKST(N, INSN, R) static void N(U8 *p, const U8 *s, const U8 *m) { A("vmovups (%[s]),%%ymm0\n vmovdqu (%[m]),%%ymm1\n" IP "1: " INSN " %%" R "0,%%" R "1,(%[p])" : GIP : [p] "r"(p), [s] "r"(s), [m] "r"(m) : "r11", "ymm0", "ymm1", "memory"); }
MASKST(r_vpmaskmovd_ymm, "vpmaskmovd", "ymm")
MASKST(r_vpmaskmovq_ymm, "vpmaskmovq", "ymm")
MASKST(r_vmaskmovps_ymm, "vmaskmovps", "ymm")
MASKST(r_vmaskmovpd_ymm, "vmaskmovpd", "ymm")
MASKST(r_vpmaskmovd_xmm, "vpmaskmovd", "xmm")
MASKST(r_vpmaskmovq_xmm, "vpmaskmovq", "xmm")
MASKST(r_vmaskmovps_xmm, "vmaskmovps", "xmm")
MASKST(r_vmaskmovpd_xmm, "vmaskmovpd", "xmm")
#define CVT(N, IMM, R, DST) static void N(U8 *p, const U8 *s, const U8 *m) { A("vmovups (%[s]),%%ymm0\n" IP "1: vcvtps2ph $" #IMM ",%%" R "0,(%[p])" : GIP : [p] "r"(p), [s] "r"(s) : "r11", "ymm0", "memory"); }
CVT(r_cvt_y0, 0, "ymm", 16) CVT(r_cvt_y1, 1, "ymm", 16) CVT(r_cvt_y2, 2, "ymm", 16) CVT(r_cvt_y3, 3, "ymm", 16) CVT(r_cvt_y4, 4, "ymm", 16)
CVT(r_cvt_x0, 0, "xmm", 8) CVT(r_cvt_x3, 3, "xmm", 8)
static void r_clwb(U8 *p, const U8 *s, const U8 *m) { A(IP "1: .byte 0x66,0x0f,0xae,0x30" : GIP : "a"(p) : "r11", "memory"); }
static void r_clflushopt(U8 *p, const U8 *s, const U8 *m) { A(IP "1: .byte 0x66,0x0f,0xae,0x38" : GIP : "a"(p) : "r11", "memory"); }
static void r_clflush(U8 *p, const U8 *s, const U8 *m) { A(IP "1: clflush (%[p])" : GIP : [p] "r"(p) : "r11", "memory"); }
static void r_prefetcht0(U8 *p, const U8 *s, const U8 *m) { A(IP "1: prefetcht0 (%[p])" : GIP : [p] "r"(p) : "r11"); }
static void r_prefetchw(U8 *p, const U8 *s, const U8 *m) { A(IP "1: prefetchw (%[p])" : GIP : [p] "r"(p) : "r11"); }
static void r_rep_movsb(U8 *p, const U8 *s, const U8 *m) {
    void *d = p; const void *sp = s; size_t n = 64;
    A(IP "1: rep movsb" : GIP, "+D"(d), "+S"(sp), "+c"(n) : : "r11", "memory");
}
static void r_rep_stosq(U8 *p, const U8 *s, const U8 *m) {
    void *d = p; size_t n = 8; uint64_t v = *(const uint64_t *)s;
    A(IP "1: rep stosq" : GIP, "+D"(d), "+c"(n) : "a"(v) : "r11", "memory");
}
static void r_lock_or32(U8 *p, const U8 *s, const U8 *m) { A(IP "1: lock orl $0x40,(%[p])" : GIP : [p] "r"(p) : "r11", "cc", "memory"); }
static void r_lock_bts32(U8 *p, const U8 *s, const U8 *m) { A(IP "1: lock btsl $3,(%[p])" : GIP : [p] "r"(p) : "r11", "cc", "memory"); }
static void r_xchg32(U8 *p, const U8 *s, const U8 *m) { uint32_t v = *(const uint32_t *)s; A(IP "1: xchgl %[v],(%[p])" : GIP, [v] "+r"(v) : [p] "r"(p) : "r11", "memory"); }

// model 5 (rmw): expected bytes computed in main (see exp_rmw)
static Case cases[] = {
 {"mov32", 2, 4, 4, 0, 1, 2, 0, 0, r_mov32, 0}, {"mov64", 2, 8, 8, 0, 1, 4, 0, 0, r_mov64, 0},
 {"vmovdqu_xmm", 2, 16, 16, 0, 1, 8, 0, 0, r_vmovdqu_xmm, 0}, {"vmovups_xmm", 2, 16, 16, 0, 1, 8, 0, 0, r_vmovups_xmm, 0}, {"movups_xmm", 2, 16, 16, 0, 1, 8, 0, 0, r_movups_xmm, 0},
 {"vmovdqu_ymm", 2, 32, 32, 0, 1, 16, 0, 0, r_vmovdqu_ymm, 0}, {"vmovups_ymm", 2, 32, 32, 0, 1, 16, 0, 0, r_vmovups_ymm, 0},
 {"vmovdqa_ymm", 2, 32, 32, 0, 32, 32, 0, 0, r_vmovdqa_ymm, 0}, {"vmovaps_ymm", 2, 32, 32, 0, 32, 32, 0, 0, r_vmovaps_ymm, 0},
 {"vextractf128_mem", 2, 16, 16, 0, 1, 8, 0, 0, r_vextractf128, 2}, {"vextracti128_mem", 2, 16, 16, 0, 1, 8, 0, 0, r_vextracti128, 2},
 {"vpmaskmovd_ymm", 2, 32, 4, 8, 1, 16, 0, 0, r_vpmaskmovd_ymm, 0}, {"vpmaskmovq_ymm", 2, 32, 8, 4, 1, 16, 0, 0, r_vpmaskmovq_ymm, 0},
 {"vmaskmovps_ymm", 2, 32, 4, 8, 1, 16, 0, 0, r_vmaskmovps_ymm, 0}, {"vmaskmovpd_ymm", 2, 32, 8, 4, 1, 16, 0, 0, r_vmaskmovpd_ymm, 0},
 {"vpmaskmovd_xmm", 2, 16, 4, 4, 1, 8, 0, 0, r_vpmaskmovd_xmm, 0}, {"vpmaskmovq_xmm", 2, 16, 8, 2, 1, 8, 0, 0, r_vpmaskmovq_xmm, 0},
 {"vmaskmovps_xmm", 2, 16, 4, 4, 1, 8, 0, 0, r_vmaskmovps_xmm, 0}, {"vmaskmovpd_xmm", 2, 16, 8, 2, 1, 8, 0, 0, r_vmaskmovpd_xmm, 0},
 {"vcvtps2ph_mem_ymm0", 2, 16, 2, 0, 1, 8, 1, 0, r_cvt_y0, 3}, {"vcvtps2ph_mem_ymm1", 2, 16, 2, 0, 1, 8, 1, 0, r_cvt_y1, 3},
 {"vcvtps2ph_mem_ymm2", 2, 16, 2, 0, 1, 8, 1, 0, r_cvt_y2, 3}, {"vcvtps2ph_mem_ymm3", 2, 16, 2, 0, 1, 8, 1, 0, r_cvt_y3, 3}, {"vcvtps2ph_mem_ymm4", 2, 16, 2, 0, 1, 8, 1, 0, r_cvt_y4, 3},
 {"vcvtps2ph_mem_xmm0", 2, 8, 2, 0, 1, 4, 1, 0, r_cvt_x0, 4}, {"vcvtps2ph_mem_xmm3", 2, 8, 2, 0, 1, 4, 1, 0, r_cvt_x3, 4},
 {"movnti32", 2, 4, 4, 0, 1, 2, 0, 0, r_movnti32, 0}, {"movnti64", 2, 8, 8, 0, 1, 4, 0, 0, r_movnti64, 0},
 {"vmovntdq_ymm", 2, 32, 32, 0, 32, 32, 0, 0, r_vmovntdq_ymm, 0}, {"vmovntps_ymm", 2, 32, 32, 0, 32, 32, 0, 0, r_vmovntps_ymm, 0},
 {"vmovntdq_xmm", 2, 16, 16, 0, 16, 16, 0, 0, r_vmovntdq_xmm, 0}, {"movntdq_xmm", 2, 16, 16, 0, 16, 16, 0, 0, r_movntdq_xmm, 0}, {"movntps_xmm", 2, 16, 16, 0, 16, 16, 0, 0, r_movntps_xmm, 0},
 {"clwb", 1, 1, 1, 0, 1, 0, 0, 4, r_clwb, 1}, {"clflushopt", 1, 1, 1, 0, 1, 0, 0, 0, r_clflushopt, 1}, {"clflush", 1, 1, 1, 0, 1, 0, 0, 0, r_clflush, 1},
 {"prefetcht0", 0, 1, 1, 0, 1, 0, 0, 0, r_prefetcht0, 1}, {"prefetchw", 0, 1, 1, 0, 1, 0, 0, 0, r_prefetchw, 1},
 {"rep_movsb", 2, 64, 1, 0, 1, 32, 0, 0, r_rep_movsb, 0}, {"rep_stosq", 2, 64, 8, 0, 1, 32, 0, 0, r_rep_stosq, 6},
 {"lock_or32", 2, 4, 4, 0, 1, 2, 0, 0, r_lock_or32, 5}, {"lock_bts32", 2, 4, 4, 0, 1, 2, 0, 0, r_lock_bts32, 7}, {"xchg32", 2, 4, 4, 0, 1, 2, 0, 0, r_xchg32, 8},
};
#define NCASES ((int)(sizeof cases / sizeof *cases))

// ---------- region setup ----------
typedef struct { int mode; int b, gran, back; const char *mask; } Cfg;
enum { M_INPAGE, M_FAULT, M_RO, M_CROSS, M_CROSSRO, M_CROSSFIRST, M_CROSSFIRSTRO, M_SPLIT };
static const char *mode_names[] = {"inpage", "fault", "ro", "cross", "crossro", "crossfirst", "crossfirstro", "split"};
static int g_initial_u32 = 0x0F0F0F0F;
// protected range [pb, pb+gran) in region offsets; returns -1 for none. prot = PAGE_NOACCESS / PAGE_READONLY
static void region_setup(const Cfg *c, size_t *pb, DWORD *prot, size_t *poff) {
    DWORD old; *pb = (size_t)-1; *prot = PAGE_NOACCESS;
    switch (c->mode) {
    case M_INPAGE: *poff = 0x2000 + 64; return;
    case M_FAULT: *pb = c->b; *poff = c->b; break;
    case M_RO: *pb = c->b; *poff = c->b; *prot = PAGE_READONLY; break;
    case M_CROSS: *pb = c->b; *poff = c->b - c->back; break;
    case M_CROSSRO: *pb = c->b; *poff = c->b - c->back; *prot = PAGE_READONLY; break;
    case M_CROSSFIRST: *pb = c->b - c->gran; *poff = c->b - c->back; break;
    case M_SPLIT: *poff = c->b - c->back; return;          // crosses the boundary, nothing protected
    case M_CROSSFIRSTRO: *pb = c->b - c->gran; *poff = c->b - c->back; *prot = PAGE_READONLY; break;
    }
    (void)old;
}
static void protect_now(size_t pb, size_t gran, DWORD prot) { DWORD old; if (pb != (size_t)-1 && !VirtualProtect(g_base + pb, gran, prot, &old)) { say("RES setup VirtualProtect failed %lu\n", GetLastError()); ExitProcess(8); } }

// first byte address (offset in region) of the enabled ranges that falls in the protected range, or -1
static long first_fault(const int *en, int nl, int lane, size_t poff, size_t pb, size_t gran) {
    if (pb == (size_t)-1) return -1;
    long best = -1;
    for (int i = 0; i < nl; i++) if (en[i]) {
        size_t s = poff + (size_t)i * lane, e = s + lane, lo = s > pb ? s : pb, hi = e < pb + gran ? e : pb + gran;
        if (lo < hi && (best < 0 || (long)lo < best)) best = (long)lo;
    }
    return best;
}
static int lane_en(const char *m, int i, int n) {
    if (!strcmp(m, "full")) return 1; if (!strcmp(m, "zero")) return 0; if (!strcmp(m, "lo")) return i < n / 2; if (!strcmp(m, "hi")) return i >= n / 2;
    if (!strcmp(m, "l0")) return i == 0; if (!strcmp(m, "last")) return i == n - 1; if (!strcmp(m, "alt")) return (i & 1) == 0; return 1;
}
static const char *addr_class(uintptr_t va, long exp_off, size_t poff, int bad) {
    if (bad) return "outside"; if (exp_off < 0) return "n/a";
    uintptr_t e = (uintptr_t)g_base + exp_off; (void)poff;
    if (va == e) return "exact"; if ((va & ~(uintptr_t)0xFFF) == (e & ~(uintptr_t)0xFFF)) return "page"; return "wrong";
}
static const char *type_name(ULONG_PTR t) { return t == 0 ? "read" : t == 1 ? "write" : t == 8 ? "exec" : "?"; }

static int cmp_img(const U8 *exp, size_t from, size_t to) {
    DWORD old; VirtualProtect(g_base, g_size, PAGE_READWRITE, &old);
    for (size_t i = from; i < to; i++) if (g_base[i] != exp[i]) return (int)i;
    return -1;
}
static void float_src(U8 *s) {
    static const float v[8] = {1.0f, -2.5f, 65504.0f, 65520.0f, 6.1035156e-5f, 5.9604645e-8f, 0.1f, 3.14159265f};
    memcpy(s, v, 32);
}

// One RES line. exp_type: type the fault must report (1 write, 0 read). regs_bad: caller's own register/flag check failed.
static void report(const char *name, const Cfg *c, int exp_fault, int exp_type, long exp_off, size_t poff, const U8 *exp_img, int regs_bad, const char *extra) {
    int nf = 0, ill = 0, bad_ip = 0; Rec *first = NULL;
    for (int i = 0; i < g_nrec && i < 12; i++) {
        if (g_rec[i].code == EXCEPTION_ACCESS_VIOLATION) { nf++; if (!first) first = &g_rec[i]; if (g_rec[i].exaddr != g_ip || g_rec[i].rip != g_ip) bad_ip = 1; }
        else ill++;
    }
    const char *ac = "n/a", *tn = "n/a"; (void)poff;
    if (first) { ac = addr_class((uintptr_t)first->va, exp_off, poff, g_nbad_va != 0); tn = type_name(first->type); }
    int bad = cmp_img(exp_img, 0, g_size);
    char why[200] = ""; int ok = 1;
    if (nf < exp_fault) { strcat(why, " missed_fault"); ok = 0; }
    if (nf > exp_fault) { strcat(why, " extra_faults"); ok = 0; }
    if (ill) { strcat(why, " illegal_instruction"); ok = 0; }
    if (bad >= 0) { strcat(why, " data"); ok = 0; }
    if (regs_bad) { strcat(why, " regs"); ok = 0; }
    if (first && bad_ip) { strcat(why, " ip"); ok = 0; }
    if (first && first->type != (ULONG_PTR)exp_type) { strcat(why, " access_type"); ok = 0; }
    if (first && exp_off >= 0 && strcmp(ac, "exact") && strcmp(ac, "page")) { strcat(why, " fault_addr"); ok = 0; }
    say("RES case=%s mode=%s back=%d b=0x%x gran=%d%s%s faults=%d exp_faults=%d ip=%s type=%s addr=%s data=%s%s status=%s%s\n", name, mode_names[c->mode], c->back, c->b, c->gran,
        c->mask ? " mask=" : "", c->mask ? c->mask : "", nf, exp_fault, first ? (bad_ip ? "wrong" : "ok") : "n/a", tn, ac, bad >= 0 ? "BAD" : "ok", extra ? extra : "", ok ? "PASS" : "FAIL", why);
    if (bad >= 0) say("  first differing byte at region offset 0x%x: got %02x want %02x\n", bad, g_base[bad], exp_img[bad]);
    for (int i = 0; i < g_nrec && i < 12; i++) if (g_rec[i].code != EXCEPTION_ACCESS_VIOLATION) say("  other exception code=0x%lx at %p\n", g_rec[i].code, g_rec[i].exaddr);
}

static int begin(const Cfg *cf, size_t *pb, DWORD *prot, size_t *poff, int align, const char *name) {
    DWORD old; VirtualProtect(g_base, g_size, PAGE_READWRITE, &old); memset(g_base, 0xA5, g_size);
    region_setup(cf, pb, prot, poff);
    if (align > 1 && (((uintptr_t)g_base + *poff) % align)) { say("RES case=%s mode=%s back=%d b=0x%x SKIP not applicable (the instruction needs %d-byte alignment)\n", name, mode_names[cf->mode], cf->back, cf->b, align); return 0; }
    return 1;
}
static int is_overlap_fault(int acc, DWORD prot, long ff) { return acc == 0 ? 0 : (ff >= 0 && (acc == 2 || prot == PAGE_NOACCESS)); }

static void run_store(const Case *c, const Cfg *cf) {
    size_t pb, poff; DWORD prot; if (!begin(cf, &pb, &prot, &poff, c->align, c->name)) return;
    int nl = c->mlanes ? c->mlanes : c->len / c->lane, en[64];
    for (int i = 0; i < nl; i++) en[i] = c->mlanes ? lane_en(cf->mask, i, nl) : 1;
    if (c->fsrc) float_src(g_src); else for (int i = 0; i < 64; i++) g_src[i] = (U8)(i * 7 + 0x31);
    memset(g_mask, 0, sizeof g_mask);
    if (c->mlanes) for (int i = 0; i < nl; i++) { uint64_t j = rnd(); uint64_t v = en[i] ? (j | (c->lane == 8 ? 0x8000000000000000ull : 0x80000000ull)) : (j & (c->lane == 8 ? 0x7FFFFFFFFFFFFFFFull : 0x7FFFFFFFull)); memcpy(g_mask + i * c->lane, &v, c->lane); }
    if (c->model == 5 || c->model == 8) memcpy(g_base + poff, &g_initial_u32, 4);
    if (c->model == 7) { uint32_t v = 0x0F0F0F00; memcpy(g_base + poff, &v, 4); }
    U8 *exp = malloc(g_size); memcpy(exp, g_base, g_size);
    switch (c->model) {
    case 0: for (int i = 0; i < nl; i++) if (en[i]) memcpy(exp + poff + i * c->lane, g_src + i * c->lane, c->lane); break;
    case 2: memcpy(exp + poff, g_src + 16, 16); break;
    case 3: case 4: {
        int imm = c->name[strlen(c->name) - 1] - '0'; int n = c->model == 3 ? 8 : 4;
        for (int i = 0; i < n; i++) { uint32_t f; memcpy(&f, g_src + 4 * i, 4); uint16_t h = sf_f2h(f, imm < 4 ? imm : 0); memcpy(exp + poff + 2 * i, &h, 2); }
    } break;
    case 5: { uint32_t v = g_initial_u32 | 0x40; memcpy(exp + poff, &v, 4); } break;
    case 6: for (int i = 0; i < 8; i++) memcpy(exp + poff + 8 * i, g_src, 8); break;
    case 7: { uint32_t v = 0x0F0F0F08; memcpy(exp + poff, &v, 4); } break;
    case 8: memcpy(exp + poff, g_src, 4); break;
    default: break;
    }
    int exp_fault = 0; long ff = (c->acc ? first_fault(en, nl, c->lane, poff, pb, cf->gran) : -1);
    exp_fault = is_overlap_fault(c->acc, prot, ff); if (!exp_fault) ff = -1;
    g_skip = c->skip; g_nrec = 0; g_nbad_va = 0; g_ip = 0;
    protect_now(pb, cf->gran, prot);
    c->run(g_base + poff, g_src, g_mask);
    report(c->name, cf, exp_fault, c->acc == 1 ? 0 : 1, ff, poff, exp, 0, NULL);
    free(exp);
}

// ---------- atomics ----------
typedef struct { uint64_t rax, rdx, rbx, rcx, src, fl, rax_o, rdx_o, src_o; U8 *p; } AX;
typedef void (*AxFn)(AX *);
#define CMPXCHG_FN(N, T, SFX, LOCK) static void N(AX *x) { uint64_t rax = x->rax, fl; T s = (T)x->src; \
    A("pushq $0x8d5\n popfq\n" IP "1: " LOCK "cmpxchg" SFX " %[s],(%[p])\n pushfq\n popq %[fl]" : GIP, "+a"(rax), [fl] "=&r"(fl) : [p] "r"(x->p), [s] "q"(s) : "r11", "cc", "memory"); x->rax_o = rax; x->fl = fl; }
CMPXCHG_FN(ax_cmpxchg8, uint8_t, "b", "") CMPXCHG_FN(ax_cmpxchg16, uint16_t, "w", "") CMPXCHG_FN(ax_cmpxchg32, uint32_t, "l", "") CMPXCHG_FN(ax_cmpxchg64, uint64_t, "q", "")
CMPXCHG_FN(ax_lcmpxchg8, uint8_t, "b", "lock ") CMPXCHG_FN(ax_lcmpxchg16, uint16_t, "w", "lock ") CMPXCHG_FN(ax_lcmpxchg32, uint32_t, "l", "lock ") CMPXCHG_FN(ax_lcmpxchg64, uint64_t, "q", "lock ")
#define XADD_FN(N, T, SFX, LOCK) static void N(AX *x) { uint64_t fl; T s = (T)x->src; \
    A("pushq $0x8d5\n popfq\n" IP "1: " LOCK "xadd" SFX " %[s],(%[p])\n pushfq\n popq %[fl]" : GIP, [s] "+q"(s), [fl] "=&r"(fl) : [p] "r"(x->p) : "r11", "cc", "memory"); x->src_o = s; x->fl = fl; }
XADD_FN(ax_xadd8, uint8_t, "b", "") XADD_FN(ax_xadd16, uint16_t, "w", "") XADD_FN(ax_xadd32, uint32_t, "l", "") XADD_FN(ax_xadd64, uint64_t, "q", "")
XADD_FN(ax_lxadd8, uint8_t, "b", "lock ") XADD_FN(ax_lxadd16, uint16_t, "w", "lock ") XADD_FN(ax_lxadd32, uint32_t, "l", "lock ") XADD_FN(ax_lxadd64, uint64_t, "q", "lock ")
static void ax_cmpxchg16b(AX *x) { uint64_t rax = x->rax, rdx = x->rdx, fl;
    A("pushq $0x8d5\n popfq\n" IP "1: lock cmpxchg16b (%[p])\n pushfq\n popq %[fl]" : GIP, "+a"(rax), "+d"(rdx), [fl] "=&r"(fl) : [p] "D"(x->p), "b"(x->rbx), "c"(x->rcx) : "r11", "cc", "memory");
    x->rax_o = rax; x->rdx_o = rdx; x->fl = fl; }
typedef struct { const char *name; int bits, kind; AxFn f; } Ac;       // kind 0 cmpxchg, 1 xadd, 2 cmpxchg16b
static Ac acs[] = {
 {"cmpxchg8", 8, 0, ax_cmpxchg8}, {"cmpxchg16", 16, 0, ax_cmpxchg16}, {"cmpxchg32", 32, 0, ax_cmpxchg32}, {"cmpxchg64", 64, 0, ax_cmpxchg64},
 {"lock_cmpxchg8", 8, 0, ax_lcmpxchg8}, {"lock_cmpxchg16", 16, 0, ax_lcmpxchg16}, {"lock_cmpxchg32", 32, 0, ax_lcmpxchg32}, {"lock_cmpxchg64", 64, 0, ax_lcmpxchg64},
 {"xadd8", 8, 1, ax_xadd8}, {"xadd16", 16, 1, ax_xadd16}, {"xadd32", 32, 1, ax_xadd32}, {"xadd64", 64, 1, ax_xadd64},
 {"lock_xadd8", 8, 1, ax_lxadd8}, {"lock_xadd16", 16, 1, ax_lxadd16}, {"lock_xadd32", 32, 1, ax_lxadd32}, {"lock_xadd64", 64, 1, ax_lxadd64},
 {"lock_cmpxchg16b", 128, 2, ax_cmpxchg16b},
};
#define NAC ((int)(sizeof acs / sizeof *acs))
static int g_rax_note;   // 32-bit cmpxchg hit: 1 RAX upper kept, 2 zeroed
static const char *verdict(uint32_t got, uint32_t arch, uint32_t rev, char *buf) {
    got &= ARITH; if (got == arch) return "x86"; if (got == rev) return "reversed"; sprintf(buf, "other(got=0x%x,x86=0x%x)", got, arch); return buf;
}
// computes expected effect of one cmpxchg/xadd; returns number of bad items; fills verdict text
static int ax_check(const Ac *a, AX *x, uint64_t M, uint64_t Ain, uint64_t S, int hit, uint64_t memfinal, const char **fv, char *vb) {
    int bits = a->bits, bad = 0; uint64_t m = msk(bits);
    if (a->kind == 0) {
        uint64_t exp_mem = hit ? (S & m) : (M & m), exp_rax = Ain;
        if (!hit) { if (bits == 64) exp_rax = M; else if (bits == 32) exp_rax = M & 0xFFFFFFFFull; else exp_rax = (Ain & ~m) | (M & m); }
        uint32_t arch = fl_sub(Ain & m, M & m, bits), rev = fl_sub(M & m, Ain & m, bits);
        g_rax_note = 0;
        if (hit && bits == 32 && x->rax_o == (Ain & 0xFFFFFFFFull) && x->rax_o != Ain) { g_rax_note = 2; exp_rax = x->rax_o; } else if (hit && bits == 32) g_rax_note = 1;
        if ((memfinal & m) != exp_mem) bad |= 1; if (x->rax_o != exp_rax) bad |= 2; if (((x->fl ^ arch) & 0x40) != 0) bad |= 4;
        *fv = verdict((uint32_t)x->fl, arch, rev, vb);
    } else {
        uint64_t exp_mem = (M + S) & m; uint32_t arch = fl_add(M & m, S & m, bits);
        if ((memfinal & m) != exp_mem) bad |= 1; if ((x->src_o & m) != (M & m)) bad |= 2;
        *fv = verdict((uint32_t)x->fl, arch, ~0u, vb);
    }
    return bad;
}
static void run_atomic(const Ac *a, const Cfg *cf, int hit) {
    size_t pb, poff; DWORD prot; int bytes = a->bits / 8;
    if (!begin(cf, &pb, &prot, &poff, a->kind == 2 ? 16 : 1, a->name)) return;
    char nm[80]; sprintf(nm, "%s_%s", a->name, a->kind == 2 || a->kind == 0 ? (hit ? "hit" : "miss") : "x");
    AX x; memset(&x, 0, sizeof x); x.p = g_base + poff;
    int en[1] = {1}; const uint64_t M = 0x8877665544332211ull, S = 0x123456789ABCDEF0ull;
    uint64_t Ain = 0, garb = 0xDEADBEEFCAFE0000ull; int bits = a->bits; uint64_t m = bits >= 64 ? ~0ull : msk(bits);
    U8 *exp = NULL;
    if (a->kind == 2) {
        uint64_t lo = M, hi = 0x1122334455667788ull; memcpy(g_base + poff, &lo, 8); memcpy(g_base + poff + 8, &hi, 8);
        x.rax = hit ? lo : lo ^ 1; x.rdx = hi; x.rbx = 0xAAAA5555AAAA5555ull; x.rcx = 0x0123456789ABCDEFull;
        exp = malloc(g_size); memcpy(exp, g_base, g_size);
        if (hit) { memcpy(exp + poff, &x.rbx, 8); memcpy(exp + poff + 8, &x.rcx, 8); }
        long ff = first_fault(en, 1, 16, poff, pb, cf->gran); int ef = is_overlap_fault(2, prot, ff); if (!ef) ff = -1;
        g_skip = 0; g_nrec = 0; g_nbad_va = 0; g_ip = 0; protect_now(pb, cf->gran, prot);
        a->f(&x);
        int bad = 0; if (x.rax_o != lo || x.rdx_o != hi) bad = 1;
        uint32_t expfl = hit ? ARITH : (ARITH & ~0x40u); char buf[64];
        if (((uint32_t)x.fl & ARITH) != expfl) bad = 1;
        sprintf(buf, " flags=%s", ((uint32_t)x.fl & ARITH) == expfl ? "x86(only ZF changes)" : "other"); sprintf(buf + strlen(buf), "(got=0x%x,x86=0x%x)", (uint32_t)x.fl & ARITH, expfl);
        report(nm, cf, ef, 1, ff, poff, exp, bad, buf);
        free(exp); return;
    }
    uint64_t Mm = M & m, Sm = S & m;
    if (a->kind == 0) { Ain = hit ? ((bits == 64 ? 0 : (garb & ~m)) | Mm) : ((bits == 64 ? 0 : (garb & ~m)) | ((Mm ^ (1ull << (bits - 1))) & m)); }
    x.rax = Ain; x.src = Sm;
    if (a->kind == 1) x.src = (bits == 64 ? 0 : (garb & ~m)) | Sm;
    memcpy(g_base + poff, &Mm, bytes);
    exp = malloc(g_size); memcpy(exp, g_base, g_size);
    uint64_t emem = a->kind == 0 ? (hit ? Sm : Mm) : ((Mm + Sm) & m); memcpy(exp + poff, &emem, bytes);
    long ff = first_fault(en, 1, bytes, poff, pb, cf->gran); int ef = is_overlap_fault(2, prot, ff); if (!ef) ff = -1;
    g_skip = 0; g_nrec = 0; g_nbad_va = 0; g_ip = 0; protect_now(pb, cf->gran, prot);
    a->f(&x);
    DWORD old; VirtualProtect(g_base, g_size, PAGE_READWRITE, &old);
    uint64_t mf = 0; memcpy(&mf, g_base + poff, bytes);
    const char *fv; char vb[64], ex[160]; int bad = ax_check(a, &x, Mm, Ain, Sm, hit, mf, &fv, vb);
    char dbg[120] = ""; if (bad) sprintf(dbg, " dbg(mem=%llx rax_o=%llx src_o=%llx fl=%llx)", (unsigned long long)mf, (unsigned long long)x.rax_o, (unsigned long long)x.src_o, (unsigned long long)x.fl);
    sprintf(ex, " flags=%s%s%s%s", fv, bad ? " (regs/mem wrong)" : "", g_rax_note == 2 ? " rax_upper_on_hit=zeroed(SDM:unchanged)" : "", dbg); if (a->kind == 0 && bad == 4) sprintf(ex, " flags=%s ZF_WRONG", fv);
    report(nm, cf, ef, 1, ff, poff, exp, bad, ex);
    free(exp);
}
static void run_cmpxchg16b_unaligned(const Cfg *cf) {
    size_t pb, poff; DWORD prot; if (!begin(cf, &pb, &prot, &poff, 1, "cmpxchg16b_unaligned")) return;
    AX x; memset(&x, 0, sizeof x); x.p = g_base + 0x2000 + 8; x.rax = 1; x.rdx = 2; x.rbx = 3; x.rcx = 4;
    g_skip = 5; g_skip_always = 1; g_nrec = 0; g_nbad_va = 0; ax_cmpxchg16b(&x); g_skip = 0; g_skip_always = 0;
    say("RES case=cmpxchg16b_unaligned mode=inpage info exceptions=%ld code=0x%lx type=%llu va=0x%llx (x86 raises #GP: Windows AV with va -1) status=INFO\n", g_nrec, g_nrec ? g_rec[0].code : 0, g_nrec ? (unsigned long long)g_rec[0].type : 0ull, g_nrec ? (unsigned long long)g_rec[0].va : 0ull);
}

// ---------- page4k / cpuid ----------
static void t_page4k(void) {
    SYSTEM_INFO si; GetSystemInfo(&si); DWORD old; MEMORY_BASIC_INFORMATION mi;
    VirtualProtect(g_base, g_size, PAGE_READWRITE, &old); memset(g_base, 0, g_size);
    VirtualProtect(g_base + 0x1000, 0x1000, PAGE_NOACCESS, &old);     // 4 KiB hole inside a 16 KiB host page
    VirtualQuery(g_base, &mi, sizeof mi); size_t r0 = mi.RegionSize;
    VirtualQuery(g_base + 0x1000, &mi, sizeof mi); size_t r1 = mi.RegionSize; DWORD p1 = mi.Protect;
    g_nrec = 0; g_nbad_va = 0; g_skip = 0; ((volatile U8 *)g_base)[0x0FFF] = 1; ((volatile U8 *)g_base)[0x2000] = 2; int nb = g_nrec;
    g_ip = (void *)1;
    ((volatile U8 *)g_base)[0x1000] = 3; int nf = g_nrec - nb;
    uintptr_t va = nf ? (uintptr_t)g_rec[nb].va : 0;
    VirtualQuery(g_base + 0x1000, &mi, sizeof mi); DWORD p1b = mi.Protect;
    int ok = si.dwPageSize == 4096 && r0 == 0x1000 && r1 == 0x1000 && p1 == PAGE_NOACCESS && nb == 0 && nf == 1 && va == (uintptr_t)g_base + 0x1000 && p1b == PAGE_READWRITE && g_base[0x1000] == 3;
    say("RES case=page4k mode=n/a dwPageSize=%lu allocgran=%lu region0=0x%zx region1=0x%zx prot_hole=0x%lx neighbour_faults=%d hole_faults=%d fault_va=%s reopened_prot=0x%lx status=%s\n", si.dwPageSize, si.dwAllocationGranularity, r0, r1, p1, nb, nf,
        va == (uintptr_t)g_base + 0x1000 ? "exact" : "wrong", p1b, ok ? "PASS" : "FAIL");
}
static void t_cpuid(void) {
    unsigned a, b, c, d; char v[13]; __cpuid(0, a, b, c, d); memcpy(v, &b, 4); memcpy(v + 4, &d, 4); memcpy(v + 8, &c, 4); v[12] = 0; unsigned maxl = a;
    __cpuid(1, a, b, c, d); say("RES case=cpuid vendor=%s maxleaf=%u leaf1.ecx=0x%x (fma=%d osxsave=%d avx=%d f16c=%d popcnt=%d cx16=%d) edx=0x%x\n", v, maxl, c, !!(c & (1 << 12)), !!(c & (1 << 27)), !!(c & (1 << 28)), !!(c & (1 << 29)), !!(c & (1 << 23)), !!(c & (1 << 13)), d);
    __cpuid_count(7, 0, a, b, c, d); say("RES case=cpuid leaf7.ebx=0x%x (avx2=%d bmi1=%d bmi2=%d erms=%d avx512f=%d clflushopt=%d clwb=%d sha=%d)\n", b, !!(b & 32), !!(b & 8), !!(b & 256), !!(b & 512), !!(b & 65536), !!(b & (1 << 23)), !!(b & (1 << 24)), !!(b & (1 << 29)));
    __cpuid(0x80000001, a, b, c, d); say("RES case=cpuid leaf80000001.ecx=0x%x (lzcnt=%d)\n", c, !!(c & 32));
    uint32_t lo, hi; A("xgetbv" : "=a"(lo), "=d"(hi) : "c"(0)); say("RES case=cpuid xcr0=0x%x%08x\n", hi, lo);
}

// ---------- numeric batches ----------
static uint32_t get_mxcsr(void) { uint32_t v; A("stmxcsr %0" : "=m"(v)); return v; }
static void set_mxcsr(uint32_t v) { A("ldmxcsr %0" : : "m"(v)); }
static void with_rm(int rm) { set_mxcsr(0x1F80u | ((uint32_t)rm << 13)); }       // MXCSR.RC = rm
static int batch_line(const char *name, int rm, long n, long bad, const char *first) {
    say("RES batch=%s rm=%d n=%ld mismatches=%ld status=%s%s%s\n", name, rm, n, bad, bad ? "FAIL" : "PASS", bad && first[0] ? " first: " : "", bad ? first : ""); return bad != 0;
}
// integer-only input generators (no FPU use while MXCSR is changed)
static uint32_t gen32(void) {
    uint32_t x = (uint32_t)rnd(); uint64_t k = rnd() % 10;
    static const uint32_t edge[] = {0, 0x80000000u, 0x3F800000u, 0xBF800000u, 0x7F800000u, 0xFF800000u, 0x00000001u, 0x007FFFFFu, 0x00800000u, 0x7F7FFFFFu, 0xFF7FFFFFu, 0x33800000u, 0x40000000u, 0x3F000000u, 0x4B000000u};
    switch (k) {
    case 0: case 1: x = (x & 0x807FFFFFu) | ((uint32_t)(120 + rnd() % 16) << 23); break;
    case 2: x &= 0x807FFFFFu; break;
    case 3: x = edge[rnd() % (sizeof edge / 4)]; break;
    case 4: x = (x & 0x807FFFFFu) | ((uint32_t)(rnd() % 255) << 23); break;
    case 5: x = (x & 0x80000000u) | (uint32_t)(rnd() % 0x01000000u); break;
    default: break;
    }
    if (sf_isnan(F32, x)) x = 0x3F800000u;
    return x;
}
static uint64_t gen64(void) {
    uint64_t x = rnd(); uint64_t k = rnd() % 10;
    static const uint64_t edge[] = {0, 0x8000000000000000ull, 0x3FF0000000000000ull, 0xBFF0000000000000ull, 0x7FF0000000000000ull, 0xFFF0000000000000ull, 1ull, 0x000FFFFFFFFFFFFFull, 0x0010000000000000ull, 0x7FEFFFFFFFFFFFFFull, 0x4330000000000000ull};
    switch (k) {
    case 0: case 1: x = (x & 0x800FFFFFFFFFFFFFull) | ((uint64_t)(1015 + rnd() % 16) << 52); break;
    case 2: x &= 0x800FFFFFFFFFFFFFull; break;
    case 3: x = edge[rnd() % (sizeof edge / 8)]; break;
    case 4: x = (x & 0x800FFFFFFFFFFFFFull) | ((rnd() % 2047) << 52); break;
    case 5: x = (x & 0x8000000000000000ull) | (rnd() % 0x0100000000000000ull >> 4); break;
    default: break;
    }
    if (sf_isnan(F64, x)) x = 0x3FF0000000000000ull;
    return x;
}
typedef void (*FmaFn)(const void *, const void *, const void *, void *);
typedef struct { const char *name; FmaFn f; int dbl, scal, form, negp, negc, asub; } Fo;
#define FMBODY(INSN, R) A("vmovups (%1),%%ymm0\n vmovups (%2),%%ymm1\n vmovups (%3),%%ymm2\n " INSN " %%" R "2,%%" R "1,%%" R "0\n vmovups %%ymm0,(%0)" :: "r"(d), "r"(a), "r"(b), "r"(c) : "ymm0", "ymm1", "ymm2", "memory")
#define FN(OP, FORM, SFX, D, S, R, NP, NC, AS) static void fn_##OP##FORM##SFX(const void *a, const void *b, const void *c, void *d) { FMBODY("vf" #OP #FORM #SFX, R); }
#define PERFORM(X, OP, SFX, D, S, R, NP, NC, AS) X(OP, 132, SFX, D, S, R, NP, NC, AS) X(OP, 213, SFX, D, S, R, NP, NC, AS) X(OP, 231, SFX, D, S, R, NP, NC, AS)
#define PERSFX(X, OP, NP, NC) PERFORM(X, OP, ps, 0, 0, "ymm", NP, NC, 0) PERFORM(X, OP, pd, 1, 0, "ymm", NP, NC, 0) PERFORM(X, OP, ss, 0, 1, "xmm", NP, NC, 0) PERFORM(X, OP, sd, 1, 1, "xmm", NP, NC, 0)
#define ALLOPS(X) PERSFX(X, madd, 0, 0) PERSFX(X, msub, 0, 1) PERSFX(X, nmadd, 1, 0) PERSFX(X, nmsub, 1, 1) \
    PERFORM(X, maddsub, ps, 0, 0, "ymm", 0, 0, 1) PERFORM(X, maddsub, pd, 1, 0, "ymm", 0, 0, 1) PERFORM(X, msubadd, ps, 0, 0, "ymm", 0, 0, 2) PERFORM(X, msubadd, pd, 1, 0, "ymm", 0, 0, 2)
ALLOPS(FN)
#define ENT(OP, FORM, SFX, D, S, R, NP, NC, AS) {"vf" #OP #FORM #SFX, fn_##OP##FORM##SFX, D, S, FORM, NP, NC, AS},
static Fo fos[] = { ALLOPS(ENT) };
#define NFO ((int)(sizeof fos / sizeof *fos))
static int batch_fma(int rmsel, int only231) {
    int fails = 0;
    for (int rm = 0; rm < 4; rm++) {
        if (rmsel >= 0 && rm != rmsel) continue;
        for (int k = 0; k < NFO; k++) {
            Fo *o = &fos[k]; if ((rm != 0 || only231) && o->form != 231) continue;
            long N = rm == 0 ? 100000 : 30000, bad = 0, n = 0; char first[200] = ""; Fmt f = o->dbl ? F64 : F32;
            int nl = o->scal ? 1 : (o->dbl ? 4 : 8), tot = o->dbl ? 4 : 8;
            _Alignas(32) uint64_t X[3][4]; _Alignas(32) uint32_t X32[3][8]; _Alignas(32) uint64_t D[4]; _Alignas(32) uint32_t D32[8];
            for (long it = 0; it < N; it++) {
                uint64_t a[8], b[8], c[8];
                for (int l = 0; l < tot; l++) {
                    a[l] = o->dbl ? gen64() : gen32(); b[l] = o->dbl ? gen64() : gen32(); c[l] = o->dbl ? gen64() : gen32();
                    if (rnd() % 4 == 0) { c[l] = sf_fma(f, a[l], b[l], 0, 1, 0, 0); if (sf_unpack(f, c[l]).cls == 2) c[l] = o->dbl ? 0x3FF0000000000000ull : 0x3F800000u; c[l] += (uint64_t)((int)(rnd() % 5) - 2) & (o->dbl ? 0xFFFFFFFFFFFFFFFFull : 0xFFFFFFFFull); if (!o->dbl) c[l] &= 0xFFFFFFFFull; if (sf_isnan(f, c[l])) c[l] = 0x3F800000u; }
                }
                // place a,b,c into the operand registers for this form: 132: X0*X2+X1  213: X1*X0+X2  231: X1*X2+X0
                int ia = o->form == 132 ? 0 : 1, ib = o->form == 132 ? 2 : (o->form == 213 ? 0 : 2), ic = o->form == 132 ? 1 : (o->form == 213 ? 2 : 0);
                for (int l = 0; l < tot; l++) {
                    if (o->dbl) { X[ia][l] = a[l]; X[ib][l] = b[l]; X[ic][l] = c[l]; } else { X32[ia][l] = (uint32_t)a[l]; X32[ib][l] = (uint32_t)b[l]; X32[ic][l] = (uint32_t)c[l]; }
                }
                with_rm(rm);
                if (o->dbl) o->f(X[0], X[1], X[2], D); else o->f(X32[0], X32[1], X32[2], D32);
                set_mxcsr(0x1F80);
                for (int l = 0; l < nl; l++) {
                    int np = o->negp, nc = o->negc; if (o->asub == 1) nc = (l % 2 == 0); if (o->asub == 2) nc = (l % 2 == 1);
                    uint64_t e = sf_fma(f, a[l], b[l], c[l], np, nc, rm), g = o->dbl ? D[l] : D32[l]; n++;
                    int zero_tie = 0; (void)zero_tie;
                    if (g != e) { if (!bad++) sprintf(first, "lane %d a=%llx b=%llx c=%llx got=%llx want=%llx", l, (unsigned long long)a[l], (unsigned long long)b[l], (unsigned long long)c[l], (unsigned long long)g, (unsigned long long)e); }
                }
                if (o->scal) {     // upper lanes: dest-register (X0) lanes kept, bits 255:128 zero
                    int per = o->dbl ? 2 : 4;
                    for (int l = 1; l < tot; l++) { uint64_t g = o->dbl ? D[l] : D32[l], e = l < per ? (o->dbl ? X[0][l] : X32[0][l]) : 0; if (g != e && !bad++) sprintf(first, "scalar upper lane %d got=%llx want=%llx", l, (unsigned long long)g, (unsigned long long)e); }
                }
            }
            char nm[40]; sprintf(nm, "%s", o->name); fails += batch_line(nm, rm, n, bad, first);
        }
    }
    return fails;
}
// FMA NaN handling: exactly one NaN input -> that NaN, quieted; invalid operations -> default NaN
static int batch_fma_nan(void) {
    _Alignas(32) uint32_t X[3][8], D[8]; long n = 0, bad = 0; char first[160] = "";
    static const uint32_t qn = 0x7FC00001u, sn = 0x7F800001u, nsn = 0xFFA00005u;
    for (int pos = 0; pos < 3; pos++) for (int t = 0; t < 3; t++) {
        uint32_t nanv = t == 0 ? qn : (t == 1 ? sn : nsn);
        for (int l = 0; l < 8; l++) { X[0][l] = 0x40000000u; X[1][l] = 0x40400000u; X[2][l] = 0x40800000u; X[pos][l] = nanv; }
        fn_madd231ps(X[0], X[1], X[2], D);      // X1*X2 + X0
        uint32_t want = nanv | 0x400000u; n++; if (D[0] != want && !bad++) sprintf(first, "NaN %08x in operand reg %d: got %08x want %08x", nanv, pos, D[0], want);
    }
    uint32_t inf = 0x7F800000u, z = 0, one = 0x3F800000u; _Alignas(32) uint32_t Y[3][8];
    struct { uint32_t a, b, c; } inv[] = {{inf, z, one}, {z, inf, one}, {inf, one, 0xFF800000u}, {0xFF800000u, one, inf}};
    for (int i = 0; i < 4; i++) { for (int l = 0; l < 8; l++) { Y[1][l] = inv[i].a; Y[2][l] = inv[i].b; Y[0][l] = inv[i].c; } fn_madd231ps(Y[0], Y[1], Y[2], D); n++; if (D[0] != 0xFFC00000u && !bad++) sprintf(first, "invalid op %d: got %08x want ffc00000", i, D[0]); }
    return batch_line("fma_nan_rules", 0, n, bad, first);
}

// FTZ / DAZ (MXCSR bits 15 / 6) on vfmadd231ps/pd: inputs flushed with DAZ, subnormal results flushed with FTZ
static int batch_fma_ftz(void) {
    int fails = 0; _Alignas(32) uint32_t X[3][8], D[8]; _Alignas(32) uint64_t Xd[3][4], Dd[4];
    for (int mode = 1; mode < 4; mode++) for (int dbl = 0; dbl < 2; dbl++) {
        uint32_t cs = 0x1F80u | ((mode & 1) ? 0x40u : 0) | ((mode & 2) ? 0x8000u : 0); long n = 0, bad = 0; char first[160] = ""; Fmt f = dbl ? F64 : F32; int tot = dbl ? 4 : 8;
        for (long it = 0; it < 30000; it++) {
            uint64_t a[8], b[8], c[8];
            for (int l = 0; l < tot; l++) {
                uint64_t *v[3] = {&a[l], &b[l], &c[l]};
                for (int k = 0; k < 3; k++) {
                    uint64_t x = dbl ? gen64() : gen32(); uint64_t sg = dbl ? 0x8000000000000000ull : 0x80000000ull, frac = dbl ? 0x000FFFFFFFFFFFFFull : 0x007FFFFFull;
                    switch (rnd() % 4) { case 0: x = (x & sg) | (rnd() & frac); break; case 1: x = (x & (sg | frac)) | ((uint64_t)(dbl ? 511 + rnd() % 8 : 63 + rnd() % 8) << (dbl ? 52 : 23)); break; default: break; }
                    if (sf_isnan(f, x)) x = dbl ? 0x3FF0000000000000ull : 0x3F800000u; *v[k] = x;
                }
                if (dbl) { Xd[1][l] = a[l]; Xd[2][l] = b[l]; Xd[0][l] = c[l]; } else { X[1][l] = (uint32_t)a[l]; X[2][l] = (uint32_t)b[l]; X[0][l] = (uint32_t)c[l]; }
            }
            set_mxcsr(cs); if (dbl) fn_madd231pd(Xd[0], Xd[1], Xd[2], Dd); else fn_madd231ps(X[0], X[1], X[2], D); set_mxcsr(0x1F80);
            for (int l = 0; l < tot; l++) {
                uint64_t ea = a[l], eb = b[l], ec = c[l];
                if (mode & 1) { uint64_t *w[3] = {&ea, &eb, &ec}; for (int k = 0; k < 3; k++) { Unp u = sf_unpack(f, *w[k]); if (u.cls == 1 && ((*w[k] >> f.mb) & ((1u << f.eb) - 1)) == 0) *w[k] = (uint64_t)u.sign << (f.eb + f.mb); } }
                uint64_t e = sf_fma(f, ea, eb, ec, 0, 0, 0);
                if (mode & 2) {   // x86: tiny = the exact result rounded to the format's precision with unbounded exponent is below the smallest normal
                    sf_unb = 1; sf_unb_eres = 0x7fffffff; (void)sf_fma(f, ea, eb, ec, 0, 0, 0); sf_unb = 0;
                    if (sf_unb_eres < 1 - ((1 << (f.eb - 1)) - 1)) { uint64_t z = sf_fma(f, ea, eb, ec, 0, 0, 3); uint64_t sg = (sf_unb_eres == -(1 << 30)) ? (e & (1ull << (f.eb + f.mb))) : 0;
                        if (sf_unb_eres != -(1 << 30)) { uint64_t d1 = sf_fma(f, ea, eb, ec, 0, 0, 1), u1 = sf_fma(f, ea, eb, ec, 0, 0, 2); uint64_t sm = (1ull << (f.eb + f.mb)) - 1; sg = ((d1 & sm) ? d1 : u1) & (1ull << (f.eb + f.mb)); (void)z; }
                        e = sg; }
                }
                uint64_t g = dbl ? Dd[l] : D[l]; n++;
                if (g != e && !bad++) sprintf(first, "a=%llx b=%llx c=%llx got=%llx want=%llx", (unsigned long long)a[l], (unsigned long long)b[l], (unsigned long long)c[l], (unsigned long long)g, (unsigned long long)e);
            }
        }
        char nm[40]; sprintf(nm, "vfmadd231%s_%s", dbl ? "pd" : "ps", mode == 1 ? "DAZ" : mode == 2 ? "FTZ" : "FTZ_DAZ"); fails += batch_line(nm, 0, n, bad, first);
    }
    return fails;
}
// ---- F16C ----
static int batch_f16c(void) {
    int fails = 0; _Alignas(32) uint16_t h[8]; _Alignas(32) uint32_t f[8], o[8]; _Alignas(16) uint16_t ho[8]; long n = 0, bad = 0; char first[160] = "";
    for (uint32_t base = 0; base < 65536; base += 8) {
        for (int i = 0; i < 8; i++) h[i] = (uint16_t)(base + i);
        A("vmovdqu (%0),%%xmm1\n vcvtph2ps %%xmm1,%%ymm0\n vmovups %%ymm0,(%1)" :: "r"(h), "r"(o) : "ymm0", "xmm1", "memory");
        for (int i = 0; i < 8; i++) { n++; uint32_t e = sf_h2f(h[i]); if (o[i] != e && !bad++) sprintf(first, "half %04x got %08x want %08x", h[i], o[i], e); }
    }
    fails += batch_line("vcvtph2ps_all_65536", 0, n, bad, first);
    for (int imm = 0; imm <= 4; imm++) for (int rc = 0; rc < (imm == 4 ? 4 : 1); rc++) {
        n = 0; bad = 0; first[0] = 0; int rm = imm < 4 ? imm : rc;
        for (long it = 0; it < 400000; it++) {
            for (int i = 0; i < 8; i++) {
                uint32_t x; uint64_t k = rnd() % 4;
                if (k == 0) x = (uint32_t)rnd();
                else if (k == 1) { x = sf_h2f((uint16_t)rnd()); x += (uint32_t)((int)(rnd() % 0x4001) - 0x2000); }
                else if (k == 2) { static const uint32_t tie[] = {0x1000, 0x0FFF, 0x1001, 0x0000, 0x0001, 0x1FFF, 0x2000, 0x3000}; x = sf_h2f((uint16_t)rnd()) & ~0x1FFFu; x |= tie[rnd() % 8]; }
                else { x = (uint32_t)(rnd() >> 16) & 0xFFFF0000u; x |= (uint32_t)(rnd() % 8) * 0x1000u + (uint32_t)(rnd() % 3) - 1; }
                f[i] = x;
            }
            with_rm(rc);
            switch (imm) {
            case 0: A("vmovups (%0),%%ymm0\n vcvtps2ph $0,%%ymm0,%%xmm1\n vmovdqu %%xmm1,(%1)" :: "r"(f), "r"(ho) : "ymm0", "xmm1", "memory"); break;
            case 1: A("vmovups (%0),%%ymm0\n vcvtps2ph $1,%%ymm0,%%xmm1\n vmovdqu %%xmm1,(%1)" :: "r"(f), "r"(ho) : "ymm0", "xmm1", "memory"); break;
            case 2: A("vmovups (%0),%%ymm0\n vcvtps2ph $2,%%ymm0,%%xmm1\n vmovdqu %%xmm1,(%1)" :: "r"(f), "r"(ho) : "ymm0", "xmm1", "memory"); break;
            case 3: A("vmovups (%0),%%ymm0\n vcvtps2ph $3,%%ymm0,%%xmm1\n vmovdqu %%xmm1,(%1)" :: "r"(f), "r"(ho) : "ymm0", "xmm1", "memory"); break;
            default: A("vmovups (%0),%%ymm0\n vcvtps2ph $4,%%ymm0,%%xmm1\n vmovdqu %%xmm1,(%1)" :: "r"(f), "r"(ho) : "ymm0", "xmm1", "memory"); break;
            }
            set_mxcsr(0x1F80);
            for (int i = 0; i < 8; i++) { n++; uint16_t e = sf_f2h(f[i], rm); if (ho[i] != e && !bad++) sprintf(first, "float %08x got %04x want %04x", f[i], ho[i], e); }
        }
        char nm[40]; sprintf(nm, "vcvtps2ph_imm%d", imm); fails += batch_line(nm, rm, n, bad, first);
    }
    // register-source vcvtps2ph xmm -> xmm (4 floats)
    return fails;
}
// ---- BMI / LZCNT / POPCNT ----
static uint64_t pdep_ref(uint64_t src, uint64_t mask, int bits) { uint64_t r = 0; int k = 0; for (int i = 0; i < bits; i++) if ((mask >> i) & 1) { if ((src >> k) & 1) r |= 1ull << i; k++; } return r; }
static uint64_t pext_ref(uint64_t src, uint64_t mask, int bits) { uint64_t r = 0; int k = 0; for (int i = 0; i < bits; i++) if ((mask >> i) & 1) { if ((src >> i) & 1) r |= 1ull << k; k++; } return r; }
static uint32_t szf(uint64_t r, int bits) { return (r ? 0 : 0x40) | ((r >> (bits - 1)) & 1 ? 0x80 : 0); }
static uint64_t gi(int bits) {
    uint64_t x = rnd(); switch (rnd() % 8) { case 0: return 0; case 1: return ~0ull & msk(bits); case 2: return 1ull << (rnd() % bits); case 3: return (~0ull << (rnd() % bits)) & msk(bits); case 4: return x & rnd() & rnd() & msk(bits); case 5: return (x | rnd() | rnd()) & msk(bits); default: return x & msk(bits); }
}
#define BM1(N, T, INSN) static void N(T s, T *r, uint64_t *f) { A("pushq $0x8d5\n popfq\n " INSN " %2,%0\n pushfq\n popq %1" : "=&r"(*r), "=&r"(*f) : "r"(s) : "cc"); }
#define BM2C(N, T, INSN) static void N(T s1, T s2, T *r, uint64_t *f) { A("pushq $0x8d5\n popfq\n " INSN " %2,%3,%0\n pushfq\n popq %1" : "=&r"(*r), "=&r"(*f) : "r"(s1), "r"(s2) : "cc"); }
#define BM2(N, T, INSN) static void N(T s1, T s2, T *r, uint64_t *f) { A("pushq $0x8d5\n popfq\n " INSN " %3,%2,%0\n pushfq\n popq %1" : "=&r"(*r), "=&r"(*f) : "r"(s1), "r"(s2) : "cc"); }
BM1(b_blsi32, uint32_t, "blsi") BM1(b_blsi64, uint64_t, "blsi") BM1(b_blsr32, uint32_t, "blsr") BM1(b_blsr64, uint64_t, "blsr") BM1(b_blsmsk32, uint32_t, "blsmsk") BM1(b_blsmsk64, uint64_t, "blsmsk")
BM1(b_tzcnt16, uint16_t, "tzcnt") BM1(b_tzcnt32, uint32_t, "tzcnt") BM1(b_tzcnt64, uint64_t, "tzcnt") BM1(b_lzcnt16, uint16_t, "lzcnt") BM1(b_lzcnt32, uint32_t, "lzcnt") BM1(b_lzcnt64, uint64_t, "lzcnt")
BM1(b_popcnt16, uint16_t, "popcnt") BM1(b_popcnt32, uint32_t, "popcnt") BM1(b_popcnt64, uint64_t, "popcnt")
// AT&T operand order: andn m,b,a  (a = ~b & m);  bextr ctl,src,dst;  bzhi idx,src,dst;  shlx cnt,src,dst;  pdep mask,src,dst
BM2(b_andn32, uint32_t, "andn") BM2(b_andn64, uint64_t, "andn")
BM2C(b_bextr32, uint32_t, "bextr") BM2C(b_bextr64, uint64_t, "bextr") BM2C(b_bzhi32, uint32_t, "bzhi") BM2C(b_bzhi64, uint64_t, "bzhi")
BM2C(b_shlx32, uint32_t, "shlx") BM2C(b_shlx64, uint64_t, "shlx") BM2C(b_shrx32, uint32_t, "shrx") BM2C(b_shrx64, uint64_t, "shrx") BM2C(b_sarx32, uint32_t, "sarx") BM2C(b_sarx64, uint64_t, "sarx")
BM2C(b_pdep32, uint32_t, "pdep") BM2C(b_pdep64, uint64_t, "pdep") BM2C(b_pext32, uint32_t, "pext") BM2C(b_pext64, uint64_t, "pext")
static void b_mulx32(uint32_t s1, uint32_t s2, uint32_t *lo, uint32_t *hi, uint64_t *f) { A("pushq $0x8d5\n popfq\n mulx %4,%1,%0\n pushfq\n popq %2" : "=&r"(*hi), "=&r"(*lo), "=&r"(*f) : "d"(s1), "r"(s2) : "cc"); }
static void b_mulx64(uint64_t s1, uint64_t s2, uint64_t *lo, uint64_t *hi, uint64_t *f) { A("pushq $0x8d5\n popfq\n mulx %4,%1,%0\n pushfq\n popq %2" : "=&r"(*hi), "=&r"(*lo), "=&r"(*f) : "d"(s1), "r"(s2) : "cc"); }
#define BR(N, T, IMM) static void N(T s, T *r, uint64_t *f) { A("pushq $0x8d5\n popfq\n rorx $" #IMM ",%2,%0\n pushfq\n popq %1" : "=&r"(*r), "=&r"(*f) : "r"(s) : "cc"); }
BR(b_rorx32_1, uint32_t, 1) BR(b_rorx32_13, uint32_t, 13) BR(b_rorx32_31, uint32_t, 31) BR(b_rorx64_1, uint64_t, 1) BR(b_rorx64_13, uint64_t, 13) BR(b_rorx64_63, uint64_t, 63)
static int batch_bmi(void) {
    int fails = 0; long N = 60000;
#define CHK1(NAME, T, BITS, FN, REFEXPR, FLEXPR, MASK) { long n = 0, bad = 0; char first[120] = ""; for (long i = 0; i < N; i++) { uint64_t s = gi(BITS); T r; uint64_t f; FN((T)s, &r, &f); uint64_t e = (REFEXPR); uint32_t ef = (FLEXPR); n++; \
        if (((uint64_t)r != (e & msk(BITS))) || (((f ^ ef) & (MASK)) != 0)) { if (!bad++) sprintf(first, "src=%llx got=%llx/fl%llx want=%llx/fl%x", (unsigned long long)s, (unsigned long long)r, (unsigned long long)(f & ARITH), (unsigned long long)(e & msk(BITS)), ef); } } fails += batch_line(NAME, 0, n, bad, first); }
    uint64_t sm; (void)sm;
#define M32 msk(32)
    CHK1("blsi32", uint32_t, 32, b_blsi32, s & (0 - s), (s ? 1 : 0) | szf(s & (0 - s), 32), 0x8C1)
    CHK1("blsi64", uint64_t, 64, b_blsi64, s & (0 - s), (s ? 1 : 0) | szf(s & (0 - s), 64), 0x8C1)
    CHK1("blsr32", uint32_t, 32, b_blsr32, s & (s - 1), (s ? 0 : 1) | szf(s & (s - 1) & M32, 32), 0x8C1)
    CHK1("blsr64", uint64_t, 64, b_blsr64, s & (s - 1), (s ? 0 : 1) | szf(s & (s - 1), 64), 0x8C1)
    CHK1("blsmsk32", uint32_t, 32, b_blsmsk32, s ^ (s - 1), (s ? 0 : 1) | szf((s ^ (s - 1)) & M32, 32) & ~0x40u, 0x8C1)
    CHK1("blsmsk64", uint64_t, 64, b_blsmsk64, s ^ (s - 1), (s ? 0 : 1) | szf(s ^ (s - 1), 64) & ~0x40u, 0x8C1)
    CHK1("tzcnt16", uint16_t, 16, b_tzcnt16, s ? __builtin_ctzll(s) : 16, (s ? 0 : 1) | (s ? (__builtin_ctzll(s) ? 0 : 0x40) : 0), 0x41)
    CHK1("tzcnt32", uint32_t, 32, b_tzcnt32, s ? __builtin_ctzll(s) : 32, (s ? 0 : 1) | (s ? (__builtin_ctzll(s) ? 0 : 0x40) : 0), 0x41)
    CHK1("tzcnt64", uint64_t, 64, b_tzcnt64, s ? __builtin_ctzll(s) : 64, (s ? 0 : 1) | (s ? (__builtin_ctzll(s) ? 0 : 0x40) : 0), 0x41)
    CHK1("lzcnt16", uint16_t, 16, b_lzcnt16, s ? __builtin_clzll(s) - 48 : 16, (s ? 0 : 1) | ((s ? __builtin_clzll(s) - 48 : 16) ? 0 : 0x40), 0x41)
    CHK1("lzcnt32", uint32_t, 32, b_lzcnt32, s ? __builtin_clzll(s) - 32 : 32, (s ? 0 : 1) | ((s ? __builtin_clzll(s) - 32 : 32) ? 0 : 0x40), 0x41)
    CHK1("lzcnt64", uint64_t, 64, b_lzcnt64, s ? __builtin_clzll(s) : 64, (s ? 0 : 1) | ((s ? __builtin_clzll(s) : 64) ? 0 : 0x40), 0x41)
    CHK1("popcnt16", uint16_t, 16, b_popcnt16, __builtin_popcountll(s), s ? 0 : 0x40, ARITH)
    CHK1("popcnt32", uint32_t, 32, b_popcnt32, __builtin_popcountll(s), s ? 0 : 0x40, ARITH)
    CHK1("popcnt64", uint64_t, 64, b_popcnt64, __builtin_popcountll(s), s ? 0 : 0x40, ARITH)
#define CHK2(NAME, T, BITS, FN, REFEXPR, FLEXPR, MASK) { long n = 0, bad = 0; char first[140] = ""; for (long i = 0; i < N; i++) { uint64_t s1 = gi(BITS), s2 = gi(BITS); if (rnd() % 3 == 0) s2 = (s2 & ~0xFFull) | (rnd() % (BITS + 3)); T r; uint64_t f; FN((T)s1, (T)s2, &r, &f); uint64_t e = (REFEXPR) & msk(BITS); uint32_t ef = (FLEXPR); n++; \
        if (((uint64_t)r != e) || (((f ^ ef) & (MASK)) != 0)) { if (!bad++) sprintf(first, "s1=%llx s2=%llx got=%llx/fl%llx want=%llx/fl%x", (unsigned long long)s1, (unsigned long long)s2, (unsigned long long)r, (unsigned long long)(f & ARITH), (unsigned long long)e, ef); } } fails += batch_line(NAME, 0, n, bad, first); }
    // b_andn(s1,s2): asm "andn s2,s1,r" => r = ~s1 & s2
#define EXT(S, C, BITS) ({ uint64_t st = (C) & 0xFF, ln = ((C) >> 8) & 0xFF; uint64_t v = st >= (BITS) ? 0 : ((S) >> st); if (ln < (BITS)) v &= (1ull << ln) - 1; v & msk(BITS); })
    CHK2("andn32", uint32_t, 32, b_andn32, ~s1 & s2, szf(~s1 & s2 & M32, 32), 0x8C1)
    CHK2("andn64", uint64_t, 64, b_andn64, ~s1 & s2, szf(~s1 & s2, 64), 0x8C1)
    CHK2("bextr32", uint32_t, 32, b_bextr32, EXT(s2, s1, 32), (EXT(s2, s1, 32) ? 0 : 0x40), 0x841)
    CHK2("bextr64", uint64_t, 64, b_bextr64, EXT(s2, s1, 64), (EXT(s2, s1, 64) ? 0 : 0x40), 0x841)
    CHK2("bzhi32", uint32_t, 32, b_bzhi32, ((s1 & 0xFF) < 32 ? (s2 & ((1ull << (s1 & 0xFF)) - 1)) : s2), ((s1 & 0xFF) > 31 ? 1 : 0) | szf(((s1 & 0xFF) < 32 ? (s2 & ((1ull << (s1 & 0xFF)) - 1)) : s2) & M32, 32), 0x8C1)
    CHK2("bzhi64", uint64_t, 64, b_bzhi64, ((s1 & 0xFF) < 64 ? (s2 & ((1ull << (s1 & 0xFF)) - 1)) : s2), ((s1 & 0xFF) > 63 ? 1 : 0) | szf(((s1 & 0xFF) < 64 ? (s2 & ((1ull << (s1 & 0xFF)) - 1)) : s2), 64), 0x8C1)
    CHK2("shlx32", uint32_t, 32, b_shlx32, (uint32_t)s2 << (s1 & 31), ARITH, ARITH)
    CHK2("shlx64", uint64_t, 64, b_shlx64, s2 << (s1 & 63), ARITH, ARITH)
    CHK2("shrx32", uint32_t, 32, b_shrx32, (uint32_t)s2 >> (s1 & 31), ARITH, ARITH)
    CHK2("shrx64", uint64_t, 64, b_shrx64, s2 >> (s1 & 63), ARITH, ARITH)
    CHK2("sarx32", uint32_t, 32, b_sarx32, (uint32_t)((int32_t)s2 >> (s1 & 31)), ARITH, ARITH)
    CHK2("sarx64", uint64_t, 64, b_sarx64, (uint64_t)((int64_t)s2 >> (s1 & 63)), ARITH, ARITH)
    CHK2("pdep32", uint32_t, 32, b_pdep32, pdep_ref(s2, s1, 32), ARITH, ARITH)
    CHK2("pdep64", uint64_t, 64, b_pdep64, pdep_ref(s2, s1, 64), ARITH, ARITH)
    CHK2("pext32", uint32_t, 32, b_pext32, pext_ref(s2, s1, 32), ARITH, ARITH)
    CHK2("pext64", uint64_t, 64, b_pext64, pext_ref(s2, s1, 64), ARITH, ARITH)
#define CHKR(NAME, T, BITS, FN, K) { long n = 0, bad = 0; char first[100] = ""; for (long i = 0; i < N; i++) { uint64_t s = gi(BITS); T r; uint64_t f; FN((T)s, &r, &f); uint64_t e = ((s >> K) | (s << (BITS - K))) & msk(BITS); n++; if (r != e || (f & ARITH) != ARITH) { if (!bad++) sprintf(first, "src=%llx got=%llx want=%llx", (unsigned long long)s, (unsigned long long)r, (unsigned long long)e); } } fails += batch_line(NAME, 0, n, bad, first); }
    CHKR("rorx32_1", uint32_t, 32, b_rorx32_1, 1) CHKR("rorx32_13", uint32_t, 32, b_rorx32_13, 13) CHKR("rorx32_31", uint32_t, 32, b_rorx32_31, 31)
    CHKR("rorx64_1", uint64_t, 64, b_rorx64_1, 1) CHKR("rorx64_13", uint64_t, 64, b_rorx64_13, 13) CHKR("rorx64_63", uint64_t, 64, b_rorx64_63, 63)
    { long n = 0, bad = 0; char first[120] = "";
      for (long i = 0; i < N; i++) { uint64_t s1 = gi(32), s2 = gi(32); uint32_t lo, hi; uint64_t f; b_mulx32((uint32_t)s1, (uint32_t)s2, &lo, &hi, &f); uint64_t p = s1 * s2; n++; if (lo != (uint32_t)p || hi != (uint32_t)(p >> 32) || (f & ARITH) != ARITH) if (!bad++) sprintf(first, "%llx*%llx", (unsigned long long)s1, (unsigned long long)s2); }
      fails += batch_line("mulx32", 0, n, bad, first); n = 0; bad = 0; first[0] = 0;
      for (long i = 0; i < N; i++) { uint64_t s1 = gi(64), s2 = gi(64), lo, hi, f; b_mulx64(s1, s2, &lo, &hi, &f); unsigned __int128 p = (unsigned __int128)s1 * s2; n++; if (lo != (uint64_t)p || hi != (uint64_t)(p >> 64) || (f & ARITH) != ARITH) if (!bad++) sprintf(first, "%llx*%llx", (unsigned long long)s1, (unsigned long long)s2); }
      fails += batch_line("mulx64", 0, n, bad, first); }
    return fails;
}
// ---- cmpxchg / xadd on random operands (no faults) ----
static int batch_atomic_flags(void) {
    int fails = 0; DWORD old; VirtualProtect(g_base, g_size, PAGE_READWRITE, &old);
    for (int k = 0; k < NAC; k++) {
        const Ac *a = &acs[k]; if (a->kind == 2) continue;
        long n = 0, bad = 0, x86 = 0, rev = 0, other = 0, misses = 0, zeroed = 0; char first[160] = ""; int bits = a->bits; uint64_t m = msk(bits);
        for (long i = 0; i < 20000; i++) {
            AX x; memset(&x, 0, sizeof x); x.p = g_base + 0x2040;
            uint64_t M = gi(bits), S = gi(bits), Ain = (rnd() % 4 == 0) ? M : gi(bits); int hit = Ain == M;
            if (a->kind == 0) { uint64_t g = 0xDEADBEEF00000000ull; x.rax = (bits == 64 ? 0 : (g & ~m)) | Ain; x.src = S; } else x.src = S;
            memcpy(x.p, &M, bits / 8); a->f(&x); uint64_t mf = 0; memcpy(&mf, x.p, bits / 8);
            const char *fv; char vb[64];
            int b = ax_check(a, &x, M, a->kind == 0 ? ((bits == 64 ? 0 : 0xDEADBEEF00000000ull & ~m) | Ain) : 0, S, hit, mf, &fv, vb);
            if (g_rax_note == 2) zeroed++;
            n++; if (b && !bad++) sprintf(first, "M=%llx A=%llx S=%llx code=%d", (unsigned long long)M, (unsigned long long)Ain, (unsigned long long)S, b);
            if (!hit || a->kind == 1) { misses++; if (!strcmp(fv, "x86")) x86++; else if (!strcmp(fv, "reversed")) rev++; else other++; }
        }
        char nm[60]; sprintf(nm, "%s_random", a->name);
        say("RES batch=%s n=%ld result_mismatches=%ld flag_verdict(x86/reversed/other)=%ld/%ld/%ld of %ld non-hit ops rax_upper_zeroed_on_hit=%ld status=%s\n", nm, n, bad, x86, rev, other, misses, zeroed, bad ? "FAIL" : (rev || other ? "FLAGS_DEVIATE" : "PASS"));
        fails += bad != 0;
    }
    return fails;
}


// RMW at every byte offset around cache-line and 4 KiB edges, no protection: which misaligned/split forms lose or corrupt data?
static int batch_atomic_split(void) {
    int fails = 0; DWORD old; VirtualProtect(g_base, g_size, PAGE_READWRITE, &old);
    int offs[400], no = 0; for (int i = 0; i <= 130; i++) offs[no++] = i; for (int i = 4096 - 9; i <= 4096 + 9; i++) offs[no++] = i; for (int i = 8192 - 9; i < 8192 + 2; i++) offs[no++] = i;
    for (int k = 0; k < NAC; k++) {
        const Ac *a = &acs[k]; if (a->kind == 2 || a->bits == 8) continue;
        for (int hit = 1; hit >= 0; hit--) {
            if (a->kind == 1 && !hit) continue;
            int bits = a->bits, bytes = bits / 8; uint64_t m = msk(bits); long n = 0, bad = 0; char list[400] = ""; int nl = 0;
            for (int oi = 0; oi < no; oi++) {
                for (int rep = 0; rep < 2; rep++) {
                    memset(g_base + 0x1F00, 0xA5, 0x2200 + 0x2000);
                    AX x; memset(&x, 0, sizeof x); x.p = g_base + 0x2000 + offs[oi];
                    uint64_t M = 0x8877665544332211ull & m, S = 0x123456789ABCDEF0ull & m, Ain = 0, g = 0xDEADBEEFCAFE0000ull;
                    if (a->kind == 0) Ain = hit ? M : ((M ^ (1ull << (bits - 1))) & m);
                    x.rax = (bits == 64 ? 0 : (g & ~m)) | Ain; x.src = S; if (a->kind == 1) x.src = (bits == 64 ? 0 : (g & ~m)) | S;
                    U8 before[160]; memcpy(x.p, &M, bytes); memcpy(before, x.p - 16, 32);
                    a->f(&x); uint64_t mf = 0; memcpy(&mf, x.p, bytes); const char *fv; char vb[64];
                    int b = ax_check(a, &x, M, x.rax, S, hit, mf, &fv, vb) & 3; n++;
                    // neighbours untouched?
                    U8 after[32]; memcpy(after, x.p - 16, 32); for (int j = 0; j < 32; j++) if ((j < 16 || j >= 16 + bytes) && after[j] != before[j]) b |= 8;
                    if (b) { bad++; if (nl < 60) nl += sprintf(list + nl, "%d(%d) ", offs[oi], b); }
                }
            }
            char nm[60]; sprintf(nm, "%s_%s_offsets", a->name, a->kind == 0 ? (hit ? "hit" : "miss") : "x");
            say("RES batch=%s rm=0 n=%ld mismatches=%ld status=%s%s%s\n", nm, n, bad, bad ? "FAIL" : "PASS", bad ? " first: bad offsets (code 1 mem,2 rax,8 neighbour): " : "", bad ? list : ""); fails += bad != 0;
        }
    }
    return fails;
}

#include "isa-numeric.h"
#include "isa-diff.h"
#include "isa-move.h"
#include "isa-fparith.h"
#include "isa-logic.h"
#include "isa-int.h"
#include "isa-intmove.h"
#include "isa-intsh.h"
#include "isa-compare.h"
#include "isa-shuffle.h"
#include "isa-iarith.h"
#include "isa-convert.h"
#include "isa-round.h"
#include "isa-masked.h"
#include "isa-sse.h"
#include "isa-cflow.h"

// ---------- main ----------
int main(int argc, char **argv) {
    if (argc < 2) { puts("usage: probe-wine list | cpuid | page4k | batch <fma|fma_all|fma_nan|f16c|bmi|atomic_flags> | <case> <mode> [back=N] [b=0x..] [gran=N] [mask=..]"); return 2; }
    setvbuf(stdout, NULL, _IONBF, 0);
    if (getenv("PROBE_FAST")) g_fast = atoi(getenv("PROBE_FAST"));
    if (getenv("PROBE_NEG")) g_neg = 1;
    if (!strcmp(argv[1], "list")) { for (int i = 0; i < NCASES; i++) printf("store %s %d %d\n", cases[i].name, cases[i].mlanes, cases[i].back); for (int i = 0; i < NAC; i++) printf("atomic %s\n", acs[i].name); return 0; }
    g_base = VirtualAlloc(NULL, g_size, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (!g_base) { say("RES setup VirtualAlloc failed\n"); return 3; }
    AddVectoredExceptionHandler(1, veh);
    if (!strcmp(argv[1], "cpuid")) { t_cpuid(); return 0; }
    if (!strcmp(argv[1], "page4k")) { t_page4k(); return 0; }
    if (!strcmp(argv[1], "batch") && argc > 2) {
        int f = 0;
        if (!strcmp(argv[2], "fma")) f = batch_fma(-1, 0) ; else if (!strcmp(argv[2], "fma_nan")) f = batch_fma_nan(); else if (!strcmp(argv[2], "fma_ftz")) f = batch_fma_ftz();
        else if (!strcmp(argv[2], "f16c")) f = batch_f16c(); else if (!strcmp(argv[2], "bmi")) f = batch_bmi(); else if (!strcmp(argv[2], "atomic_flags")) f = batch_atomic_flags(); else if (!strcmp(argv[2], "atomic_split")) f = batch_atomic_split();
        else if (!strcmp(argv[2], "rcp_rsqrt")) {
            const char *sel = argc > 3 ? argv[3] : NULL; struct { const char *n; int rs, sc, y; } v[] = {{"vrcpps_xmm", 0, 0, 0}, {"vrcpps_ymm", 0, 0, 1}, {"vrsqrtps_xmm", 1, 0, 0}, {"vrsqrtps_ymm", 1, 0, 1}, {"vrcpss", 0, 1, 0}, {"vrsqrtss", 1, 1, 0}};
            for (unsigned i = 0; i < sizeof v / sizeof *v; i++) if (sel_ok(sel, v[i].n)) f += batch_rcp(v[i].rs, v[i].sc, v[i].y);
        }
        else if (!strcmp(argv[2], "cvt_f2i")) f = batch_cvt_f2i(argc > 3 ? argv[3] : NULL);
        else if (!strcmp(argv[2], "cvt_i2f")) f = batch_cvt_i2f(argc > 3 ? argv[3] : NULL);
        else if (!strcmp(argv[2], "cvt_f2f")) f = batch_cvt_f2f(argc > 3 ? argv[3] : NULL);
        else if (!strcmp(argv[2], "avx_fparith")) f = batch_ops("avx_fparith", reg_fparith_ops, argc > 3 ? argv[3] : NULL, plan_fp);
        else if (!strcmp(argv[2], "avx_logic")) f = batch_ops("avx_logic", reg_logic_ops, argc > 3 ? argv[3] : NULL, plan_default);
        else if (!strcmp(argv[2], "x86_int_alu")) f = batch_ops("x86_int_alu", reg_alu_ops, argc > 3 ? argv[3] : NULL, plan_int);
        else if (!strcmp(argv[2], "x86_int_move")) f = batch_ops("x86_int_move", reg_intmove_ops, argc > 3 ? argv[3] : NULL, plan_int);
        else if (!strcmp(argv[2], "x86_int_shift_mul_div")) f = batch_ops("x86_int_shift_mul_div", reg_intsh_ops, argc > 3 ? argv[3] : NULL, plan_int);
        else if (!strcmp(argv[2], "avx_compare")) f = batch_ops("avx_compare", reg_compare_ops, argc > 3 ? argv[3] : NULL, plan_cmp);
        else if (!strcmp(argv[2], "avx_shuffle")) f = batch_ops("avx_shuffle", reg_shuffle_ops, argc > 3 ? argv[3] : NULL, plan_shuf);
        else if (!strcmp(argv[2], "x86_control_flow")) f = batch_ops("x86_control_flow", reg_cflow_ops, argc > 3 ? argv[3] : NULL, plan_cf);
        else if (!strcmp(argv[2], "avx_int_arith")) f = batch_ops("avx_int_arith", reg_iarith_ops, argc > 3 ? argv[3] : NULL, plan_ia);
        else if (!strcmp(argv[2], "avx_convert")) f = batch_ops("avx_convert", reg_convert_ops, argc > 3 ? argv[3] : NULL, plan_cv);
        else if (!strcmp(argv[2], "avx_round")) f = batch_ops("avx_round", reg_round_ops, argc > 3 ? argv[3] : NULL, plan_rnd);
        else if (!strcmp(argv[2], "avx_masked")) f = batch_ops("avx_masked", reg_masked_ops, argc > 3 ? argv[3] : NULL, plan_mask);
        else if (!strcmp(argv[2], "x86_sse_legacy")) f = batch_ops("x86_sse_legacy", reg_sse_ops, argc > 3 ? argv[3] : NULL, plan_sse);
        else if (!strcmp(argv[2], "avx_move")) f = batch_ops("avx_move", reg_move_ops, argc > 3 ? argv[3] : NULL, plan_default);
        else { say("unknown batch\n"); return 2; }
        return f ? 1 : 0;
    }
    if (argc < 3) { say("need a mode\n"); return 2; }
    Cfg cf = {0, 0x3000, 0x1000, 0, NULL}; int mode = -1;
    for (int i = 0; i < 8; i++) if (!strcmp(argv[2], mode_names[i])) mode = i;
    if (mode < 0) { say("unknown mode %s\n", argv[2]); return 2; }
    cf.mode = mode; int back_set = 0;
    for (int i = 3; i < argc; i++) {
        if (!strncmp(argv[i], "back=", 5)) { cf.back = atoi(argv[i] + 5); back_set = 1; } else if (!strncmp(argv[i], "b=", 2)) cf.b = (int)strtol(argv[i] + 2, 0, 0);
        else if (!strncmp(argv[i], "gran=", 5)) cf.gran = atoi(argv[i] + 5); else if (!strncmp(argv[i], "mask=", 5)) cf.mask = argv[i] + 5;
    }
    if (!cf.mask) cf.mask = "full";
    if (!strcmp(argv[1], "cmpxchg16b_unaligned")) { run_cmpxchg16b_unaligned(&cf); return 0; }
    for (int i = 0; i < NCASES; i++) if (!strcmp(cases[i].name, argv[1])) { if (!back_set) cf.back = cases[i].back; if (!cases[i].mlanes) cf.mask = NULL; run_store(&cases[i], &cf); return 0; }
    for (int i = 0; i < NAC; i++) if (!strncmp(acs[i].name, argv[1], strlen(acs[i].name)) && (argv[1][strlen(acs[i].name)] == 0 || argv[1][strlen(acs[i].name)] == ':')) {
        int hit = !strstr(argv[1], ":miss"); if (!back_set) cf.back = acs[i].bits == 128 ? 16 : acs[i].bits / 16; cf.mask = NULL; run_atomic(&acs[i], &cf, hit); return 0;
    }
    say("unknown case %s\n", argv[1]); return 2;
}
