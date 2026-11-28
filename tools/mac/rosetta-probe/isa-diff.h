// Generic differential engine for x86 instruction forms under Rosetta (included by probe-wine.c after isa-numeric.h, which provides
// Rep/NOTE/rep_emit, g_neg, g_fast, set_mxcsr, rnd, say, E32/E64).
//
// One "op" = one instruction form (mnemonic, operand kinds, vector width, alias pattern, immediate). Every op has
//   - a kernel: an asm loop that loads a record (dst, src1, src2, src3: 4 x 32 bytes) into ymm0..ymm3, runs the instruction, and stores the
//     destination register (always all 256 bits, so VEX.128 zeroing of the upper half is checked) and, for GPR/flag results, extra words;
//   - a model: plain C (no FPU, no x86 intrinsics) that computes the expected 32 bytes + extras from the same record.
// Inputs are random, edge-biased, "near" (src2 close to src1: equal, off by one, bit flips), mixed, and (8/16-bit element ops)
// exhaustive over all operand pairs. Alias variants (dst == src1, dst == src2, src1 == src2, all equal) and memory-source variants
// (aligned, unaligned, straddling a 4 KiB / 16 KiB page boundary) are separate ops. Immediates are stub tables (one 128-byte stub
// per value 0..255, built with .irp), so every value is its own code.
// Output: one RES line per op and MXCSR variant (rep_emit format), fam=<family>.
#ifndef ISA_DIFF_H
#define ISA_DIFF_H
#include <stddef.h>
#include "isa-types.h"
#define MODEL_NEG g_neg

// ---------------- kernels ----------------
#define K_PRE "vmovups (%0),%%ymm0\n vmovups 32(%0),%%ymm1\n vmovups 64(%0),%%ymm2\n vmovups 96(%0),%%ymm3\n"
#define K_POST "\n add $128,%0\n add $64,%1\n dec %2\n jnz 1b"
#define ST0 "vmovups %%ymm0,(%1)"
#define ST1 "vmovups %%ymm1,(%1)"
#define ST2 "vmovups %%ymm2,(%1)"
#define KRN(NAME, BODY, ST) static void NAME(const IRec *in, Out *out, long n) { \
    A("1:\n" K_PRE "\n " BODY "\n " ST K_POST : "+r"(in), "+r"(out), "+r"(n) :: "ymm0", "ymm1", "ymm2", "ymm3", "rax", "rcx", "memory", "cc"); }
// memory-destination kernels: the 64-byte output is pre-filled with d||c, the instruction writes into it at OFF
#define KST(NAME, BODY) static void NAME(const IRec *in, Out *out, long n) { \
    A("1:\n" K_PRE "\n vmovups %%ymm0,(%1)\n vmovups %%ymm3,32(%1)\n " BODY K_POST : "+r"(in), "+r"(out), "+r"(n) :: "ymm0", "ymm1", "ymm2", "ymm3", "rax", "rcx", "memory", "cc"); }

// immediate stubs: Win64 ABI (rcx = records, rdx = out, r8 = count), 128 bytes per immediate value
#define IMM256 "0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31,32,33,34,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,56,57,58,59,60,61,62,63,64,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,87,88,89,90,91,92,93,94,95,96,97,98,99,100,101,102,103,104,105,106,107,108,109,110,111,112,113,114,115,116,117,118,119,120,121,122,123,124,125,126,127,128,129,130,131,132,133,134,135,136,137,138,139,140,141,142,143,144,145,146,147,148,149,150,151,152,153,154,155,156,157,158,159,160,161,162,163,164,165,166,167,168,169,170,171,172,173,174,175,176,177,178,179,180,181,182,183,184,185,186,187,188,189,190,191,192,193,194,195,196,197,198,199,200,201,202,203,204,205,206,207,208,209,210,211,212,213,214,215,216,217,218,219,220,221,222,223,224,225,226,227,228,229,230,231,232,233,234,235,236,237,238,239,240,241,242,243,244,245,246,247,248,249,250,251,252,253,254,255"
#define STUB(NAME, BODY, ST) \
    extern const char isa_tab_##NAME[]; \
    __asm__(".text\n .balign 128\n .globl isa_tab_" #NAME "\n isa_tab_" #NAME ":\n .irp I," IMM256 "\n .balign 128\n 1:\n" \
            " vmovups (%rcx),%ymm0\n vmovups 32(%rcx),%ymm1\n vmovups 64(%rcx),%ymm2\n vmovups 96(%rcx),%ymm3\n " BODY "\n " ST "\n add $128,%rcx\n add $64,%rdx\n dec %r8\n jnz 1b\n ret\n .endr\n");
#define S_ST0 "vmovups %ymm0,(%rdx)"
#define S_ST1 "vmovups %ymm1,(%rdx)"
#define S_ST2 "vmovups %ymm2,(%rdx)"

// 3-operand AVX forms "M rm, vvvv, dst": kernels for the distinct-register form, dst==src1, dst==src2, src1==src2, all equal, and a memory source
#define G3R(M, R, W) \
    KRN(k_##M##_##W##dd, #M " %%" R "2,%%" R "1,%%" R "0", ST0) KRN(k_##M##_##W##da, #M " %%" R "2,%%" R "1,%%" R "1", ST1) \
    KRN(k_##M##_##W##db, #M " %%" R "2,%%" R "1,%%" R "2", ST2) KRN(k_##M##_##W##ab, #M " %%" R "1,%%" R "1,%%" R "0", ST0) \
    KRN(k_##M##_##W##al, #M " %%" R "1,%%" R "1,%%" R "1", ST1) KRN(k_##M##_##W##mm, #M " 64(%0),%%" R "1,%%" R "0", ST0)
#define G3(M) G3R(M, "xmm", X) G3R(M, "ymm", Y)
// 2-operand forms "M rm, dst" (source class RS, destination class RD): distinct, src==dst, memory source
#define G2R(M, RS, RD, W) \
    KRN(k_##M##_##W##dd, #M " %%" RS "2,%%" RD "0", ST0) KRN(k_##M##_##W##da, #M " %%" RS "1,%%" RD "1", ST1) KRN(k_##M##_##W##mm, #M " 64(%0),%%" RD "0", ST0)

#define MAXOPS 6000
static Op g_ops[MAXOPS]; static int g_nops;
static char *nm_(const char *a, const char *b) { size_t n = strlen(a) + strlen(b) + 1; char *s = malloc(n); snprintf(s, n, "%s%s", a, b); return s; }
static Op *newop(const char *nm, KF k, MF m, int w, int ek) {
    if (g_nops >= MAXOPS) { say("too many ops\n"); exit(3); }
    Op *o = &g_ops[g_nops++]; memset(o, 0, sizeof *o); o->name = nm; o->k = k; o->m = m; o->w = w; o->ek = ek; o->cr = 32; o->nsrc = 2; o->sz = w; return o;
}
// 3-operand op: six alias variants of one instruction at one width
static void reg3(const char *base, KF kdd, KF kda, KF kdb, KF kab, KF kal, KF kmm, int w, int ek, MF m, const void *p, int flt, int nsrc, int exh) {
    KF ks[6] = {kdd, kda, kdb, kab, kal, kmm}; const char *sfx[6] = {"", "_da", "_db", "_ab", "_all", "_mem"};
    for (int i = 0; i < 6; i++) { if (!ks[i]) continue; Op *o = newop(nm_(base, sfx[i]), ks[i], m, w, ek); o->p = p; o->alias = i; o->flt = flt; o->nsrc = nsrc; o->exh = exh && i == 0; }
}
// 2-operand op: distinct, src==dst, memory source
static void reg2(const char *base, KF kdd, KF kda, KF kmm, int w, int ek, MF m, const void *p, int flt, int sz) {
    KF ks[3] = {kdd, kda, kmm}; const char *sfx[3] = {"", "_da", "_mem"}; int al[3] = {AL_DD, AL_DA, AL_MEM};
    for (int i = 0; i < 3; i++) { if (!ks[i]) continue; Op *o = newop(nm_(base, sfx[i]), ks[i], m, w, ek); o->p = p; o->alias = al[i]; o->flt = flt; o->nsrc = 1; o->sz = sz ? sz : w; }
}
#define REG3(M, MOD, EK, P, FLT, NSRC, EXH) \
    reg3(#M "_xmm", k_##M##_Xdd, k_##M##_Xda, k_##M##_Xdb, k_##M##_Xab, k_##M##_Xal, k_##M##_Xmm, 16, EK, MOD, P, FLT, NSRC, EXH); \
    reg3(#M "_ymm", k_##M##_Ydd, k_##M##_Yda, k_##M##_Ydb, k_##M##_Yab, k_##M##_Yal, k_##M##_Ymm, 32, EK, MOD, P, FLT, NSRC, EXH);

// ---------------- input generation ----------------
static uint64_t gi_el(int bits, int stream) {
    uint64_t x = rnd() & mskb(bits);
    if (stream == 1 || (stream == 5 && (rnd() & 1))) {
        uint64_t m = mskb(bits), top = 1ull << (bits - 1);
        switch (rnd() % 14) {
        case 0: x = 0; break; case 1: x = 1; break; case 2: x = m; break; case 3: x = top; break; case 4: x = top - 1; break; case 5: x = top + 1; break;
        case 6: x = 0x5555555555555555ull & m; break; case 7: x = 0xAAAAAAAAAAAAAAAAull & m; break;
        case 8: x = rnd() % (unsigned)(bits + 2); break;                    // shift counts, around the element width
        case 9: x = (unsigned)bits + (rnd() % 3) - 1; break;
        case 10: x = (0 - (rnd() % 4)) & m; break; case 11: x = (rnd() & rnd() & rnd()) & m; break;
        case 12: x = (rnd() | rnd() | rnd()) & m; break; default: x = (1ull << (rnd() % bits)) & m; break;
        }
    }
    return x;
}
static uint32_t gf32_el(int stream) {
    uint32_t x = (uint32_t)rnd();
    switch (stream) {
    case 0: break;
    case 1: x = E32[rnd() % (sizeof E32 / 4)]; if (rnd() & 1) x ^= 0x80000000u; break;
    case 2: case 5: { uint32_t E = 127 - 24 + (uint32_t)(rnd() % 49); x = (x & 0x80000000u) | (E << 23) | ((uint32_t)rnd() & 0x7FFFFF);
        if (rnd() % 6 == 0) x = (x & 0xFF800000u) | (uint32_t[]){0, 1, 0x7FFFFF, 0x400000, 0x7FFFFE}[rnd() % 5]; break; }
    case 3: { static const uint32_t ex[] = {0, 1, 2, 3, 22, 23, 24, 25, 126, 127, 128, 252, 253, 254, 254, 255}; x = (x & 0x807FFFFFu) | (ex[rnd() % 16] << 23); break; }
    default: break;
    }
    return x;
}
static uint64_t gf64_el(int stream) {
    uint64_t x = rnd();
    switch (stream) {
    case 0: break;
    case 1: x = E64[rnd() % (sizeof E64 / 8)]; if (rnd() & 1) x ^= 0x8000000000000000ull; break;
    case 2: case 5: { uint64_t E = 1023 - 54 + (uint64_t)(rnd() % 109); x = (x & 0x8000000000000000ull) | (E << 52) | (rnd() & 0xFFFFFFFFFFFFFull);
        if (rnd() % 6 == 0) x = (x & 0xFFF0000000000000ull) | (uint64_t[]){0, 1, 0xFFFFFFFFFFFFFull, 0x8000000000000ull, 0xFFFFFFFFFFFFEull}[rnd() % 5]; break; }
    case 3: { static const uint64_t ex[] = {0, 1, 2, 3, 51, 52, 53, 54, 1022, 1023, 1024, 2044, 2045, 2046, 2046, 2047}; x = (x & 0x800FFFFFFFFFFFFFull) | (ex[rnd() % 16] << 52); break; }
    default: break;
    }
    return x;
}
// streams: 0 raw, 1 edge, 2 normal (floats: moderate exponents), 3 range (floats: exponent extremes), 4 near (src2 close to src1), 5 mix
static const char *STREAMS = "raw,edge,normal,range,near,mix";
static void gen_field(uint8_t *p, int ek, int stream) {
    int sz = ESZ[ek], ne = 32 / sz;
    for (int i = 0; i < ne; i++) {
        uint64_t v;
        int s = stream == 4 ? 5 : stream;
        if (ek == EK_F32) v = gf32_el(s == 5 ? (int)(rnd() % 4) : s); else if (ek == EK_F64) v = gf64_el(s == 5 ? (int)(rnd() % 4) : s);
        else v = gi_el(sz * 8, s == 2 || s == 3 ? 5 : s);
        st_(p + i * sz, sz, v);
    }
}
static void gen_rec(IRec *r, const Op *o, int stream) {
    int ek = o->ek;
    gen_field(r->d, ek, stream == 4 ? 5 : stream); gen_field(r->a, ek, stream == 4 ? 5 : stream); gen_field(r->b, ek, stream == 4 ? 5 : stream); gen_field(r->c, ek, stream == 4 ? 5 : stream);
    if (stream == 4) {      // src2 near src1
        int sz = ESZ[ek], ne = 32 / sz;
        for (int i = 0; i < ne; i++) {
            uint64_t a = ld_(r->a + i * sz, sz), b = ld_(r->b + i * sz, sz);
            switch (rnd() % 8) {
            case 0: case 1: case 2: b = a; break;
            case 3: b = (a + 1) & mskb(sz * 8); break; case 4: b = (a - 1) & mskb(sz * 8); break;
            case 5: b = a ^ (1ull << (rnd() % (sz * 8))); break; case 6: b = a ^ (1ull << (sz * 8 - 1)); break; default: break;
            }
            st_(r->b + i * sz, sz, b);
            if (rnd() % 4 == 0) st_(r->c + i * sz, sz, ld_(r->a + i * sz, sz)); else if (rnd() % 4 == 0) st_(r->c + i * sz, sz, b);
        }
    }
    if (o->alias == AL_AB || o->alias == AL_ALL) memcpy(r->b, r->a, 32);
    if (o->memoff && o->alias == AL_MEM) { /* the model reads at +64+memoff; the generator already filled everything the model may read */ }
}

// ---------------- running and comparing ----------------
static int g_negflip;
static void hexs(char *d, size_t n, const uint8_t *p, int len) { size_t k = 0; for (int i = len - 1; i >= 0 && k + 3 < n; i--) k += (size_t)snprintf(d + k, n - k, "%02x", p[i]); if (k < n) d[k] = 0; }
static int cmp_out(const Op *o, const Out *got, const Out *exp, int *lane) {
    int cr = o->mem_image ? 32 : o->cr;
    if (cr && memcmp(got->r, exp->r, (size_t)cr)) { for (int i = 0; i < cr; i++) if (got->r[i] != exp->r[i]) { *lane = i; return 1; } }
    for (int i = 0; i < 4; i++) if ((got->x[i] ^ exp->x[i]) & o->xm[i]) { *lane = 100 + i; return 1; }
    return 0;
}
static void describe(char *dst, size_t n, const Op *o, const IRec *rc, const Out *got, const Out *exp, int lane) {
    char a[70], b[70], c[70], g[70], e[70]; int w = o->w == 32 ? 32 : 16;
    hexs(a, sizeof a, rc->a, w); hexs(b, sizeof b, rc->b, w); hexs(c, sizeof c, rc->c, w);
    if (lane >= 100) { snprintf(dst, n, "a=%s b=%s x%d got=%llx want=%llx", a, b, lane - 100, (unsigned long long)got->x[lane - 100], (unsigned long long)exp->x[lane - 100]); return; }
    hexs(g, sizeof g, got->r, 32); hexs(e, sizeof e, exp->r, 32);
    if (o->nsrc >= 3) snprintf(dst, n, "byte%d a=%s b=%s c=%s got=%s want=%s", lane, a, b, c, g, e); else snprintf(dst, n, "byte%d a=%s b=%s got=%s want=%s", lane, a, b, g, e);
}
#define CHN 2048
static IRec *g_in; static Out *g_got, *g_exp;
static void alloc_bufs(void) { if (!g_in) { g_in = _aligned_malloc(sizeof(IRec) * CHN, 32); g_got = _aligned_malloc(sizeof(Out) * CHN, 32); g_exp = _aligned_malloc(sizeof(Out) * CHN, 32); } }
static void call_k(KF k, const IRec *in, Out *out, long n, uint32_t mx) { if (mx != 0x1F80) set_mxcsr(mx); k(in, out, n); if (mx != 0x1F80) set_mxcsr(0x1F80); }

// one op, one MXCSR value: nrec records of random streams (stream chosen per record), then the model comparison.
// exh: element-pair exhaustive pass (8-bit: every (a,b) pair in every lane position = 65536 records; 16-bit: all 2^32 pairs once).
static int g_str[CHN];
static uint64_t exh_records(const Op *o) {
    int L = o->w / ESZ[o->ek];
    return o->ek == EK_I8 ? 65536u : (1ull << 32) / (uint64_t)L;      // I16: all 2^32 pairs; F32 with one source: all 2^32 inputs
}
static void gen_exh(IRec *rc, const Op *o, uint64_t cnt) {
    int L = o->w / ESZ[o->ek], sz = ESZ[o->ek];
    gen_field(rc->c, o->ek, 5); gen_field(rc->d, o->ek, 5); memset(rc->a, 0, 32); memset(rc->b, 0, 32);
    for (int j = 0; j < 32 / sz; j++) {
        uint64_t q;
        if (o->ek == EK_F32) { st_(rc->b + j * 4, 4, cnt * (uint64_t)L + (uint64_t)(j % L)); continue; }
        if (sz == 1) { uint64_t per = 65536u / (uint64_t)L, blk = cnt % per, rot = (cnt / per) % (uint64_t)L; q = (blk * (uint64_t)L + ((uint64_t)j + rot) % (uint64_t)L) & 0xFFFF; }
        else q = (cnt * (uint64_t)L + (uint64_t)(j % L)) & 0xFFFFFFFFull;
        st_(rc->a + j * sz, sz, q & mskb(sz * 8)); st_(rc->b + j * sz, sz, (q >> (sz * 8)) & mskb(sz * 8));
    }
}
static int run_stream_op(const Op *o, uint32_t mx, long nrec, int exh, const char *tag) {
    alloc_bufs(); Rep r; rep_init(&r, "raw,edge,normal,range,near,mix,exh"); long done = 0; uint64_t cnt = 0;
    Ctx cx = {o, o->w, o->imm, mx};
    int mode_exh = exh && (o->ek == EK_I8 || o->ek == EK_I16 || (o->ek == EK_F32 && o->nsrc == 1));
    long total = mode_exh ? (long)exh_records(o) : nrec;
    total >>= g_fast; if (total < CHN) total = CHN;
    while (done < total) {
        long n = total - done < CHN ? total - done : CHN;
        for (long i = 0; i < n; i++) {
            IRec *rc = &g_in[i]; int stream;
            if (mode_exh) { stream = 6; gen_exh(rc, o, cnt++); }
            else { stream = (int)(rnd() % 8); if (stream > 5) stream = o->flt ? 4 : 5; if (!o->flt && (stream == 2 || stream == 3)) stream = 5; gen_rec(rc, o, stream); }
            g_str[i] = stream; memset(&g_exp[i], 0, sizeof(Out)); memset(&g_got[i], 0, sizeof(Out));
        }
        call_k(o->k, g_in, g_got, n, mx);
        for (long i = 0; i < n; i++) {
            Out *e = &g_exp[i];
            if (o->mem_image) { memcpy(e->r, g_in[i].d, 32); memcpy(e->x, g_in[i].c, 32); }
            o->m(&cx, &g_in[i], e);
            if (g_neg && (done + i) % 4099 == 17) { if (o->cr) e->r[(done + i) % 32] ^= 1; else e->x[0] ^= 1; }      // engine-level negative control: one wrong expectation per 4099 records
            int lane = 0; int bad = cmp_out(o, &g_got[i], e, &lane);
            char buf[320]; buf[0] = 0; if (bad) describe(buf, sizeof buf, o, &g_in[i], &g_got[i], e, lane);
            NOTE(&r, g_str[i], bad, "%s", buf);
        }
        done += n;
    }
    char mode[96]; snprintf(mode, sizeof mode, "mx=%04x%s%s%s%s", mx, (mx & 0x8000) ? "+FTZ" : "", (mx & 0x40) ? "+DAZ" : "", ((mx >> 13) & 3) ? (const char *[]){"", "+RC=down", "+RC=up", "+RC=zero"}[(mx >> 13) & 3] : "", tag ? tag : "");
    char extra[120]; snprintf(extra, sizeof extra, "records=%ld elements=%lld", done, (long long)done * (o->w / ESZ[o->ek]));
    return rep_emit(g_fam, o->name, mode, &r, extra);
}

// ---------------- memory operands straddling a page boundary ----------------
// A memory source (or destination) that starts a few bytes before a 4 KiB / 16 KiB boundary and ends after it, both pages accessible.
static int run_straddle(const Op *o) {
    Rep r; rep_init(&r, "straddle"); static uint8_t *pg; if (!pg) pg = VirtualAlloc(NULL, 0x20000, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    static const int bnd[3] = {0x4000, 0x5000, 0x8000}; Ctx cx = {o, o->w, o->imm, 0x1F80}; Out got, exp; int sz = o->sz ? o->sz : o->w;
    for (int bi = 0; bi < 3; bi++) for (int rel = -(sz + 1); rel <= 1; rel++) for (int rep = 0; rep < 600 >> (g_fast > 3 ? 3 : g_fast); rep++) {
        if (o->mem_image) {            // store: the 64-byte image lives across the boundary; the written bytes [memoff, memoff+sz) start at B+rel
            IRec rc; gen_rec(&rc, o, (int)(rnd() % 6)); Out *op = (Out *)(pg + bnd[bi] + rel - o->memoff);
            call_k(o->k, &rc, op, 1, 0x1F80); memset(&exp, 0, sizeof exp); memcpy(exp.r, rc.d, 32); memcpy(exp.x, rc.c, 32); o->m(&cx, &rc, &exp);
            memcpy(&got, op, sizeof got); int lane = 0; int bad = cmp_out(o, &got, &exp, &lane); char buf[320]; buf[0] = 0; if (bad) describe(buf, sizeof buf, o, &rc, &got, &exp, lane);
            NOTE(&r, 0, bad, "rel=%d %s", rel, buf);
        } else {
            IRec *rc = (IRec *)(pg + bnd[bi] + rel - 64 - o->memoff); gen_rec(rc, o, (int)(rnd() % 6)); memset(&exp, 0, sizeof exp); memset(&got, 0, sizeof got);
            call_k(o->k, rc, &got, 1, 0x1F80); o->m(&cx, rc, &exp); int lane = 0; int bad = cmp_out(o, &got, &exp, &lane); char buf[320]; buf[0] = 0; if (bad) describe(buf, sizeof buf, o, rc, &got, &exp, lane);
            NOTE(&r, 0, bad, "rel=%d %s", rel, buf);
        }
    }
    return rep_emit(g_fam, o->name, "straddle(4K,16K,4K-only boundary)", &r, "kind=memory_across_page_boundary");
}

// ---------------- driver ----------------
typedef int (*PlanFn)(const Op *);
static int plan_default(const Op *o) {
    int fl = 0; long n = o->alias == AL_DD ? (16L << 20) : (4L << 20);
    fl += run_stream_op(o, 0x1F80, n, o->exh, NULL);
    if ((o->alias == AL_MEM || o->mem_image) && o->al == 0) fl += run_straddle(o);
    return fl;
}
static int batch_ops(const char *fam, void (*reg)(void), const char *sel, PlanFn plan) {
    g_fam = fam; if (!g_nops) reg(); int fl = 0, nrun = 0;
    for (int i = 0; i < g_nops; i++) { if (!sel_ok(sel, g_ops[i].name)) continue; nrun++; fl += plan(&g_ops[i]); }
    if (!nrun) say("RES fam=%s batch=none mode=none n=0 mismatches=1 info_diffs=0 status=FAIL cls=\"selector matched no op\"\n", fam), fl++;
    return fl;
}
#endif
