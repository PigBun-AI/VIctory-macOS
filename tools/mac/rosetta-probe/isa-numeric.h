// Numeric differential batches for probe-wine.c: reciprocal approximations (vrcpps/vrsqrtps/vrcpss/vrsqrtss) and the
// conversion family (vcvt[t]ps2dq, vcvt[t]ss2si, vcvt[t]sd2si, vcvt[t]pd2dq, vcvtdq2ps, vcvtsi2ss/sd, vcvtdq2pd, vcvtps2pd,
// vcvtss2sd, vcvtpd2ps, vcvtsd2ss). Included by probe-wine.c (uses its A/say/rnd/set_mxcsr). Output: one
//   RES fam=<family> batch=<form> mode=<MXCSR variant> n=<elements> mismatches=<n> status=PASS|FAIL|INFO cls="<class bad/n; ...>" [first="..."]
// line per form and MXCSR variant; tools/mac/rosetta-probe/summarize-wine.py turns them into per-family verdicts.
// Kernels are asm loops over a buffer; MXCSR is changed only around a kernel call and the checks run with the default MXCSR.
#include <math.h>
#include <malloc.h>
#include "softref-cvt.h"

static int g_neg;       // PROBE_NEG=1: expectations deliberately wrong (Arm-style saturation, bound halved) - a negative control that the checks can fail
static int g_fast;      // PROBE_FAST=n divides every pass by 2^n (smoke tests only; the real run leaves it unset)
typedef void (*K)(const void *, void *, long long);
static _Alignas(32) uint32_t g_pat[8] = {0x11111111u, 0x22222222u, 0x33333333u, 0x44444444u, 0x55555555u, 0x66666666u, 0x77777777u, 0x88888888u};
static void runk(K k, const void *in, void *out, long long iters, uint32_t mx) { set_mxcsr(mx); k(in, out, iters); set_mxcsr(0x1F80); }

#define KB(NAME, PRE, LOOP, INB, OUTB, ...) \
    static void NAME(const void *in, void *out, long long n) { \
        A(PRE "\n1:\n " LOOP "\n add $" #INB ", %0\n add $" #OUTB ", %1\n dec %2\n jnz 1b" : "+r"(in), "+r"(out), "+r"(n) : "m"(g_pat) : __VA_ARGS__, "xmm2", "memory", "cc"); }
#define LDPAT "vmovups %3, %%xmm2"
// packed one-source kernels: load SRC, instruction, store DST
#define KP(NAME, LD, SRC, INSN, DST, ST, INB, OUTB) KB(NAME, LDPAT, LD " (%0), %%" SRC "\n vpcmpeqd %%xmm1, %%xmm1, %%xmm1\n " INSN " %%" SRC ", %%" DST "\n " ST " %%" DST ", (%1)", INB, OUTB, "xmm0", "xmm1", "ymm0", "ymm1")
KP(k_rcpps_y, "vmovups", "ymm0", "vrcpps", "ymm1", "vmovups", 32, 32)
KP(k_rcpps_x, "vmovups", "xmm0", "vrcpps", "xmm1", "vmovups", 16, 16)
KP(k_rsqrtps_y, "vmovups", "ymm0", "vrsqrtps", "ymm1", "vmovups", 32, 32)
KP(k_rsqrtps_x, "vmovups", "xmm0", "vrsqrtps", "xmm1", "vmovups", 16, 16)
// scalar approximations: dest lanes 1..3 come from xmm2 (g_pat), lane 0 = f(low lane of the source)
KB(k_rcpss, LDPAT, "vmovss (%0), %%xmm0\n vrcpss %%xmm0, %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 4, 16, "xmm0", "xmm1")
KB(k_rsqrtss, LDPAT, "vmovss (%0), %%xmm0\n vrsqrtss %%xmm0, %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 4, 16, "xmm0", "xmm1")
// float -> int32, packed
KP(k_tps2dq_y, "vmovups", "ymm0", "vcvttps2dq", "ymm1", "vmovups", 32, 32)
KP(k_tps2dq_x, "vmovups", "xmm0", "vcvttps2dq", "xmm1", "vmovups", 16, 16)
KP(k_ps2dq_y, "vmovups", "ymm0", "vcvtps2dq", "ymm1", "vmovups", 32, 32)
KP(k_ps2dq_x, "vmovups", "xmm0", "vcvtps2dq", "xmm1", "vmovups", 16, 16)
// float/double -> GPR
KB(k_ttss2si32, "", "vmovss (%0), %%xmm0\n vcvttss2si %%xmm0, %%eax\n mov %%eax, (%1)", 4, 4, "xmm0", "rax")
KB(k_ttss2si64, "", "vmovss (%0), %%xmm0\n vcvttss2si %%xmm0, %%rax\n mov %%rax, (%1)", 4, 8, "xmm0", "rax")
KB(k_ss2si32, "", "vmovss (%0), %%xmm0\n vcvtss2si %%xmm0, %%eax\n mov %%eax, (%1)", 4, 4, "xmm0", "rax")
KB(k_ss2si64, "", "vmovss (%0), %%xmm0\n vcvtss2si %%xmm0, %%rax\n mov %%rax, (%1)", 4, 8, "xmm0", "rax")
KB(k_ttsd2si32, "", "vmovsd (%0), %%xmm0\n vcvttsd2si %%xmm0, %%eax\n mov %%eax, (%1)", 8, 4, "xmm0", "rax")
KB(k_ttsd2si64, "", "vmovsd (%0), %%xmm0\n vcvttsd2si %%xmm0, %%rax\n mov %%rax, (%1)", 8, 8, "xmm0", "rax")
KB(k_sd2si32, "", "vmovsd (%0), %%xmm0\n vcvtsd2si %%xmm0, %%eax\n mov %%eax, (%1)", 8, 4, "xmm0", "rax")
KB(k_sd2si64, "", "vmovsd (%0), %%xmm0\n vcvtsd2si %%xmm0, %%rax\n mov %%rax, (%1)", 8, 8, "xmm0", "rax")
// double -> int32 packed (xmm source: 2 doubles, upper two dwords must be zero; ymm source: 4 doubles)
KP(k_ttpd2dq_x, "vmovups", "xmm0", "vcvttpd2dq", "xmm1", "vmovups", 16, 16)
KP(k_ttpd2dq_y, "vmovups", "ymm0", "vcvttpd2dq", "xmm1", "vmovups", 32, 16)
KP(k_pd2dq_x, "vmovups", "xmm0", "vcvtpd2dq", "xmm1", "vmovups", 16, 16)
KP(k_pd2dq_y, "vmovups", "ymm0", "vcvtpd2dq", "xmm1", "vmovups", 32, 16)
// int -> float / double
KP(k_dq2ps_y, "vmovups", "ymm0", "vcvtdq2ps", "ymm1", "vmovups", 32, 32)
KP(k_dq2ps_x, "vmovups", "xmm0", "vcvtdq2ps", "xmm1", "vmovups", 16, 16)
KB(k_si2ss32, LDPAT, "mov (%0), %%eax\n vcvtsi2ssl %%eax, %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 4, 16, "xmm1", "rax")
KB(k_si2ss64, LDPAT, "mov (%0), %%rax\n vcvtsi2ssq %%rax, %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 8, 16, "xmm1", "rax")
KB(k_si2sd32, LDPAT, "mov (%0), %%eax\n vcvtsi2sdl %%eax, %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 4, 16, "xmm1", "rax")
KB(k_si2sd64, LDPAT, "mov (%0), %%rax\n vcvtsi2sdq %%rax, %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 8, 16, "xmm1", "rax")
KB(k_dq2pd_y, "", "vmovups (%0), %%xmm0\n vcvtdq2pd %%xmm0, %%ymm1\n vmovups %%ymm1, (%1)", 16, 32, "xmm0", "ymm1")
KB(k_dq2pd_x, "", "vmovq (%0), %%xmm0\n vpcmpeqd %%xmm1, %%xmm1, %%xmm1\n vcvtdq2pd %%xmm0, %%xmm1\n vmovups %%xmm1, (%1)", 8, 16, "xmm0", "xmm1")
// float <-> double
KB(k_ps2pd_y, "", "vmovups (%0), %%xmm0\n vcvtps2pd %%xmm0, %%ymm1\n vmovups %%ymm1, (%1)", 16, 32, "xmm0", "ymm1")
KB(k_ps2pd_x, "", "vmovq (%0), %%xmm0\n vpcmpeqd %%xmm1, %%xmm1, %%xmm1\n vcvtps2pd %%xmm0, %%xmm1\n vmovups %%xmm1, (%1)", 8, 16, "xmm0", "xmm1")
KB(k_ss2sd, LDPAT, "vmovss (%0), %%xmm0\n vcvtss2sd %%xmm0, %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 4, 16, "xmm0", "xmm1")
KP(k_pd2ps_x, "vmovups", "xmm0", "vcvtpd2ps", "xmm1", "vmovups", 16, 16)
KP(k_pd2ps_y, "vmovups", "ymm0", "vcvtpd2ps", "xmm1", "vmovups", 32, 16)
KB(k_sd2ss, LDPAT, "vmovsd (%0), %%xmm0\n vcvtsd2ss %%xmm0, %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 8, 16, "xmm0", "xmm1")

// memory-operand forms (the title uses them: vcvtss2sd m32 371, vcvtsi2ss m32 871, vcvtdq2ps m128 15, vcvttss2si m32 602, vcvtpd2ps m128/m256 62 ...): same checks, source read straight from memory
#define KM(NAME, INSN, DST, ST, INB, OUTB) KB(NAME, LDPAT, "vpcmpeqd %%xmm1, %%xmm1, %%xmm1\n " INSN " (%0), %%" DST "\n " ST " %%" DST ", (%1)", INB, OUTB, "xmm1", "ymm1")
KM(m_tps2dq_y, "vcvttps2dq", "ymm1", "vmovups", 32, 32) KM(m_tps2dq_x, "vcvttps2dq", "xmm1", "vmovups", 16, 16) KM(m_ps2dq_y, "vcvtps2dq", "ymm1", "vmovups", 32, 32) KM(m_ps2dq_x, "vcvtps2dq", "xmm1", "vmovups", 16, 16)
KM(m_ttpd2dq_x, "vcvttpd2dqx", "xmm1", "vmovups", 16, 16) KM(m_ttpd2dq_y, "vcvttpd2dqy", "xmm1", "vmovups", 32, 16) KM(m_pd2dq_x, "vcvtpd2dqx", "xmm1", "vmovups", 16, 16) KM(m_pd2dq_y, "vcvtpd2dqy", "xmm1", "vmovups", 32, 16)
KB(m_ttss2si32, "", "vcvttss2si (%0), %%eax\n mov %%eax, (%1)", 4, 4, "rax") KB(m_ttss2si64, "", "vcvttss2si (%0), %%rax\n mov %%rax, (%1)", 4, 8, "rax")
KB(m_ss2si32, "", "vcvtss2si (%0), %%eax\n mov %%eax, (%1)", 4, 4, "rax") KB(m_ss2si64, "", "vcvtss2si (%0), %%rax\n mov %%rax, (%1)", 4, 8, "rax")
KB(m_ttsd2si32, "", "vcvttsd2si (%0), %%eax\n mov %%eax, (%1)", 8, 4, "rax") KB(m_ttsd2si64, "", "vcvttsd2si (%0), %%rax\n mov %%rax, (%1)", 8, 8, "rax")
KB(m_sd2si32, "", "vcvtsd2si (%0), %%eax\n mov %%eax, (%1)", 8, 4, "rax") KB(m_sd2si64, "", "vcvtsd2si (%0), %%rax\n mov %%rax, (%1)", 8, 8, "rax")
KM(m_dq2ps_y, "vcvtdq2ps", "ymm1", "vmovups", 32, 32) KM(m_dq2ps_x, "vcvtdq2ps", "xmm1", "vmovups", 16, 16)
KB(m_si2ss32, LDPAT, "vcvtsi2ssl (%0), %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 4, 16, "xmm1") KB(m_si2ss64, LDPAT, "vcvtsi2ssq (%0), %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 8, 16, "xmm1")
KB(m_si2sd32, LDPAT, "vcvtsi2sdl (%0), %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 4, 16, "xmm1") KB(m_si2sd64, LDPAT, "vcvtsi2sdq (%0), %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 8, 16, "xmm1")
KM(m_dq2pd_y, "vcvtdq2pd", "ymm1", "vmovups", 16, 32) KM(m_dq2pd_x, "vcvtdq2pd", "xmm1", "vmovups", 8, 16)
KM(m_ps2pd_y, "vcvtps2pd", "ymm1", "vmovups", 16, 32) KM(m_ps2pd_x, "vcvtps2pd", "xmm1", "vmovups", 8, 16)
KB(m_ss2sd, LDPAT, "vcvtss2sd (%0), %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 4, 16, "xmm1") KB(m_sd2ss, LDPAT, "vcvtsd2ss (%0), %%xmm2, %%xmm1\n vmovups %%xmm1, (%1)", 8, 16, "xmm1")
KM(m_pd2ps_x, "vcvtpd2psx", "xmm1", "vmovups", 16, 16) KM(m_pd2ps_y, "vcvtpd2psy", "xmm1", "vmovups", 32, 16)

// ---------------- reporting ----------------
#define MAXC 10
typedef struct { char name[24]; int info; long long n, bad; char first[360]; } Cls;
typedef struct { Cls c[MAXC]; int nc; } Rep;
static void rep_init(Rep *r, const char *names) {      // "name,name,..."; a leading '~' marks an informational class (differences do not fail)
    memset(r, 0, sizeof *r); const char *p = names;
    while (*p && r->nc < MAXC) {
        Cls *c = &r->c[r->nc++]; if (*p == '~') { c->info = 1; p++; }
        int l = 0; while (*p && *p != ',' && l < 23) c->name[l++] = *p++;
        while (*p && *p != ',') p++; if (*p == ',') p++;
    }
}
#define NOTE(R, K, BAD, ...) do { Cls *c_ = &(R)->c[K]; c_->n++; if (BAD) { c_->bad++; if (!c_->first[0]) snprintf(c_->first, sizeof c_->first, __VA_ARGS__); } } while (0)
static int rep_emit(const char *fam, const char *batch, const char *mode, Rep *r, const char *extra) {
    long long n = 0, bad = 0, ibad = 0; char cls[900] = "", first[380] = "";
    for (int i = 0; i < r->nc; i++) {
        Cls *c = &r->c[i]; if (!c->n) continue;
        if (c->info) ibad += c->bad; else { bad += c->bad; n += c->n; }
        char b[90]; snprintf(b, sizeof b, "%s%s%s %lld/%lld", cls[0] ? "; " : "", c->info ? "~" : "", c->name, c->bad, c->n); strncat(cls, b, sizeof cls - strlen(cls) - 1);
        if (c->bad && !c->info && !first[0]) snprintf(first, sizeof first, "%s: %s", c->name, c->first);
    }
    for (char *p = first; *p; p++) if (*p == '"') *p = '\'';
    say("RES fam=%s batch=%s mode=%s n=%lld mismatches=%lld info_diffs=%lld status=%s cls=\"%s\"%s%s%s%s%s\n", fam, batch, mode, n, bad, ibad, bad ? "FAIL" : "PASS", cls,
        extra && extra[0] ? " " : "", extra ? extra : "", first[0] ? " first=\"" : "", first, first[0] ? "\"" : "");
    return bad != 0;
}
static int fclass(Fmt f, uint64_t x) { Unp u = sf_unpack(f, x); int E = (int)((x >> f.mb) & ((1u << f.eb) - 1)); if (u.cls == 3) return 4; if (u.cls == 2) return 3; if (u.cls == 0) return 0; return E == 0 ? 1 : 2; }
static inline float u2f(uint32_t x) { float f; memcpy(&f, &x, 4); return f; }

// ---------------- reciprocal approximations ----------------
// Spec (Intel SDM RCPPS/RSQRTPS, AMD APM): max relative error <= 1.5 * 2^-12; independent of MXCSR rounding control; a zero or denormal
// source gives infinity of the source's sign; rcp of a huge input (tiny reciprocal) flushes to a signed zero; rsqrt of a negative
// non-zero source (and of -inf) gives the default QNaN 0xFFC00000; a NaN input gives a quiet NaN.
#define RCP_BOUND ((g_neg ? 0.5 : 1.5) / 4096.0)
static const uint32_t MXVAR[4] = {0x1F80, 0x1F80 | 0x8000, 0x1F80 | 0x40, 0x1F80 | 0x8000 | 0x40};
static const char *MXNAME[4] = {"default", "FTZ", "DAZ", "FTZ+DAZ"};
typedef struct { double maxrel; uint32_t maxx, maxy; } RrAcc;

static void rr_check(Rep *r, int rs, uint32_t x, uint32_t y, RrAcc *a) {
    int E = (x >> 23) & 255, sg = x >> 31; uint32_t m = x & 0x7FFFFF;
    if (E == 255 && m) {   // NaN
        NOTE(r, 0, !(((y >> 23) & 255) == 255 && (y & 0x7FFFFF)), "x=%08x y=%08x want a NaN", x, y);
        NOTE(r, 1, y != (x | 0x400000), "x=%08x y=%08x (payload not kept: want %08x)", x, y, x | 0x400000);
        return;
    }
    if (E == 0) {          // zero and denormal: infinity of the same sign
        uint32_t want = ((uint32_t)sg << 31) | 0x7F800000u;
        NOTE(r, m ? 3 : 2, y != want, "x=%08x y=%08x want %08x", x, y, want);
        return;
    }
    if (E == 255) {        // infinity
        uint32_t want = !rs ? ((uint32_t)sg << 31) : (sg ? 0xFFC00000u : 0u);
        NOTE(r, 4, y != want, "x=%08x y=%08x want %08x", x, y, want);
        return;
    }
    if (rs && sg) { NOTE(r, 5, y != 0xFFC00000u, "x=%08x y=%08x want ffc00000 (default NaN)", x, y); return; }
    double xd = (double)u2f(x); int yE = (y >> 23) & 255;
    if (!rs) {
        double rc = 1.0 / xd, ar = rc < 0 ? -rc : rc, tiny = 1.1754943508222875e-38;   // 2^-126
        if (ar < tiny * (1 + 1.0 / 1024)) {   // flush zone (the approximation may or may not cross 2^-126 within 2^-10 of it)
            if ((y & 0x7FFFFFFFu) == 0) { NOTE(r, 6, (y >> 31) != (uint32_t)sg, "x=%08x y=%08x flushed result lost the sign", x, y); return; }
            if (ar < tiny * (1 - 1.0 / 1024)) { NOTE(r, 6, 1, "x=%08x y=%08x want a signed zero (tiny result flushed)", x, y); return; }
        }
    }
    if (yE == 0 || yE == 255 || (y >> 31) != (uint32_t)(rs ? 0 : sg)) { NOTE(r, 7, 1, "x=%08x y=%08x not a normal number of the right sign", x, y); return; }
    double yd = (double)u2f(y), rel = rs ? fabs(yd * sqrt(xd) - 1.0) : fabs(yd * xd - 1.0);
    if (rel > a->maxrel) { a->maxrel = rel; a->maxx = x; a->maxy = y; }
    NOTE(r, 7, rel > RCP_BOUND, "x=%08x y=%08x rel_err=%.6g (%.4f x 2^-12; bound 1.5)", x, y, rel, rel * 4096.0);
}

static int batch_rcp(int rs, int scalar, int ymm) {
    char bn[40]; snprintf(bn, sizeof bn, "%s%s", rs ? "vrsqrt" : "vrcp", scalar ? "ss" : (ymm ? "ps_ymm" : "ps_xmm"));
    K k = rs ? (scalar ? k_rsqrtss : (ymm ? k_rsqrtps_y : k_rsqrtps_x)) : (scalar ? k_rcpss : (ymm ? k_rcpps_y : k_rcpps_x));
    const long long CH = 1 << 16; int lanes = scalar ? 1 : (ymm ? 8 : 4), mul = scalar ? 4 : 1;      // the scalar kernels write 16 bytes per element
    uint32_t *in = _aligned_malloc(CH * 4, 32), *out = _aligned_malloc(CH * 4 * mul, 32), *out2 = _aligned_malloc(CH * 4 * mul, 32);
    Rep r; rep_init(&r, "nan,~nan_payload,zero,denormal,inf,negative,flush,normal"); long long nx[3] = {0, 0, 0}; char nxfirst[3][120] = {"", "", ""};
    RrAcc acc = {0, 0, 0}; uint32_t *y127 = malloc(4u << 23), *y128 = malloc(4u << 23); long long upper_bad = 0;
    for (uint64_t base = 0; base < ((1ull << 32) >> g_fast); base += CH) {
        for (long long i = 0; i < CH; i++) in[i] = (uint32_t)(base + i);
        long long iters = CH / lanes;
        runk(k, in, out, iters, MXVAR[0]);
        for (long long i = 0; i < CH; i++) {
            rr_check(&r, rs, in[i], out[i * mul], &acc);
            if (scalar && (out[4 * i + 1] != g_pat[1] || out[4 * i + 2] != g_pat[2] || out[4 * i + 3] != g_pat[3])) upper_bad++;
        }
        for (int v = 1; v < 4; v++) {
            runk(k, in, out2, iters, MXVAR[v]);
            if (memcmp(out, out2, CH * 4 * mul)) for (long long i = 0; i < CH * mul; i++) if (out[i] != out2[i]) { if (!nx[v - 1]++) snprintf(nxfirst[v - 1], 120, "x=%08x default=%08x %s=%08x", in[i / mul], out[i], MXNAME[v], out2[i]); }
        }
        uint32_t hi = (uint32_t)(base >> 23);
        if (hi == 127 || hi == 128) { uint32_t *dst = hi == 127 ? y127 : y128; for (long long i = 0; i < CH; i++) dst[(base & 0x7FFFFF) + i] = out[i * mul]; }
    }
    char extra[300]; snprintf(extra, sizeof extra, "max_rel_err=%.6g max_rel_err_x2pow12=%.4f at_x=%08x y=%08x bound=1.5 scalar_upper_lanes_bad=%lld", acc.maxrel, acc.maxrel * 4096.0, acc.maxx, acc.maxy, upper_bad);
    int fails = rep_emit("rcp_rsqrt", bn, "default", &r, extra);
    if (upper_bad) fails++;
    long long nb = nx[0] + nx[1] + nx[2];
    say("RES fam=rcp_rsqrt batch=%s mode=mxcsr_independent n=%llu mismatches=%lld info_diffs=0 status=%s cls=\"FTZ %lld/4294967296; DAZ %lld/4294967296; FTZ+DAZ %lld/4294967296\"%s%s%s\n", bn, 3ull << 32, nb, nb ? "FAIL" : "PASS", nx[0], nx[1], nx[2],
        nb ? " first=\"" : "", nb ? (nxfirst[0][0] ? nxfirst[0] : (nxfirst[1][0] ? nxfirst[1] : nxfirst[2])) : "", nb ? "\"" : "");
    fails += nb != 0;
    // the shape of the approximation: distinct results over [1,2) and [2,4); smallest k such that the result depends only on the top k mantissa bits
    char sh[300] = "";
    for (int ex = 0; ex < 2; ex++) {
        uint32_t *y = ex ? y128 : y127; long long dist = 0; uint8_t *seen = calloc(1 << 22, 1);
        for (long long i = 0; i < (1 << 23); i++) { uint32_t d = y[i] - 0x3E800000u; if (d >= (1u << 25)) { dist = -1; break; } if (!(seen[d >> 3] & (1 << (d & 7)))) { seen[d >> 3] |= 1 << (d & 7); dist++; } }
        free(seen); int kk = 23;
        for (int k2 = 0; k2 <= 23; k2++) { int ok = 1; for (long long i = 0; i < (1 << 23) && ok; i++) if (y[i] != y[(i >> (23 - k2)) << (23 - k2)]) ok = 0; if (ok) { kk = k2; break; } }
        char b[120]; snprintf(b, sizeof b, "%sE%d_distinct_results=%lld E%d_result_depends_on_top_%d_mantissa_bits", ex ? " " : "", ex ? 128 : 127, dist, ex ? 128 : 127, kk); strncat(sh, b, sizeof sh - strlen(sh) - 1);
    }
    uint32_t pts[4] = {0x3F800000u, 0x40000000u, 0x40400000u, 0x3F000000u}; _Alignas(32) uint32_t inp[8], outp[16]; char vals[220] = "";
    for (int i = 0; i < 4; i++) { for (int l = 0; l < 8; l++) inp[l] = pts[i]; runk(k, inp, outp, scalar ? 1 : (ymm ? 1 : 2), MXVAR[0]); char b[40]; snprintf(b, sizeof b, "%s%08x->%08x", i ? " " : "", pts[i], outp[0]); strncat(vals, b, sizeof vals - strlen(vals) - 1); }
    say("RES fam=rcp_rsqrt batch=%s mode=shape n=0 mismatches=0 info_diffs=0 status=INFO %s values=\"%s\"\n", bn, sh, vals);
    free(y127); free(y128); _aligned_free(in); _aligned_free(out); _aligned_free(out2);
    return fails;
}

// ---------------- conversions ----------------
static const uint32_t E32[] = {0, 0x80000000u, 0x7F800000u, 0xFF800000u, 0x7FC00000u, 0xFFC00000u, 0x7F800001u, 0xFFBFFFFFu, 0x7FFFFFFFu, 0x3F800000u, 0xBF800000u, 0x3F000000u, 0xBF000000u, 0x3FC00000u, 0x4B000000u, 0x4B800000u, 0x4EFFFFFFu, 0x4F000000u, 0xCF000000u, 0xCF000001u, 0xCEFFFFFFu, 0x5F000000u, 0xDF000000u, 0x5EFFFFFFu, 0x7F7FFFFFu, 0xFF7FFFFFu, 0x00800000u, 0x007FFFFFu, 0x00000001u, 0x80000001u};
static const uint64_t E64[] = {0, 0x8000000000000000ull, 0x7FF0000000000000ull, 0xFFF0000000000000ull, 0x7FF8000000000000ull, 0xFFF8000000000000ull, 0x7FF0000000000001ull, 0xFFF7FFFFFFFFFFFFull, 0x3FF0000000000000ull, 0xBFF0000000000000ull, 0x3FE0000000000000ull, 0xBFE0000000000000ull,
    0x41DFFFFFFFC00000ull, 0x41E0000000000000ull, 0xC1E0000000000000ull, 0xC1E0000000200000ull, 0xC1DFFFFFFFE00000ull, 0x43E0000000000000ull, 0xC3E0000000000000ull, 0x43DFFFFFFFFFFFFFull, 0x47EFFFFFE0000000ull, 0x47EFFFFFFFFFFFFFull, 0x47F0000000000000ull, 0x3810000000000000ull, 0x380FFFFFFFFFFFFFull, 0x36A0000000000000ull, 0x369FFFFFFFFFFFFFull, 0x7FEFFFFFFFFFFFFFull, 0x0010000000000000ull, 0x000FFFFFFFFFFFFFull, 1ull};
static uint32_t g32(void) {      // float inputs for the sampled conversion passes
    uint32_t x = (uint32_t)rnd(); if (rnd() % 16 == 0) return E32[rnd() % (sizeof E32 / 4)];
    switch (rnd() % 10) {
    case 0: case 1: { int E = 120 + (int)(rnd() % 40); uint32_t sig = (uint32_t)rnd() & 0x7FFFFF; int sh = 150 - E; x = (x & 0x80000000u) | ((uint32_t)E << 23) | sig;
        if (sh >= 1 && sh <= 23 && (rnd() & 1)) { x &= ~((1u << sh) - 1); x |= 1u << (sh - 1); }
        if (sh >= 1 && sh <= 23 && !(rnd() & 3)) x += (uint32_t)((int)(rnd() % 3) - 1); break; }                                 // ties and their neighbours
    case 2: x &= 0x807FFFFFu; break;                                                                                          // zero / denormal
    case 3: x = (x & 0x80000000u) | (uint32_t)(0x4EFFFFF0u + rnd() % 32); break;                                              // around 2^31
    case 4: x = (x & 0x80000000u) | (uint32_t)(0x5EFFFFF0u + rnd() % 32); break;                                              // around 2^63
    case 5: x |= 0x7F800000u; break;                                                                                          // inf / nan
    case 6: x = (x & 0x80000000u) | ((uint32_t)((rnd() % 3) ? 0x3F000000u : 0x3E800000u) + (uint32_t)(rnd() % 8) - 4); break;  // around 0.5 / 0.25
    default: break;
    }
    return x;
}
static uint64_t g64(void) {      // double inputs
    uint64_t x = rnd(); if (rnd() % 16 == 0) return E64[rnd() % (sizeof E64 / 8)];
    switch (rnd() % 12) {
    case 0: case 1: case 2: { int E = 1023 - 3 + (int)(rnd() % 70); uint64_t sig = rnd() & 0xFFFFFFFFFFFFFull; int sh = 1075 - E; x = (x & 0x8000000000000000ull) | ((uint64_t)E << 52) | sig;
        if (sh >= 1 && sh <= 52 && (rnd() & 1)) { x &= ~((1ull << sh) - 1); x |= 1ull << (sh - 1); } break; }
    case 3: x &= 0x800FFFFFFFFFFFFFull; break;
    case 4: x = (x & 0x8000000000000000ull) | (0x41DFFFFFFFC00000ull + rnd() % 64) | (rnd() & 1 ? 0x0010000000000000ull : 0); break;   // around 2^31 / 2^32
    case 5: x = (x & 0x8000000000000000ull) | (0x43DFFFFFFFFFFFF0ull + rnd() % 32); break;                                      // around 2^63
    case 6: x |= 0x7FF0000000000000ull; break;
    case 7: case 8: { int E = 1023 - 160 + (int)(rnd() % 300); x = (x & 0x800FFFFFFFFFFFFFull) | ((uint64_t)E << 52);          // double -> float: range incl. float denormals/overflow
        if (rnd() % 3 == 0) x = (x & ~0x1FFFFFFFull) | 0x10000000ull; else if (rnd() % 4 == 0) x = (x & ~0x1FFFFFFFull) | (uint64_t)(0x10000000ll + (long long)(rnd() % 3) - 1); break; }
    case 9: x = (x & 0x8000000000000000ull) | (uint64_t)((long long)0x3FE0000000000000ll + (long long)(rnd() % 16) - 8); break;
    default: break;
    }
    return x;
}
static int64_t gint(int bits) {  // integer inputs: all bit lengths, and values near the float rounding ties
    uint64_t x = rnd(); int len = (int)(rnd() % (unsigned)(bits + 1)); if (len < 64) x &= (len ? ((1ull << len) - 1) : 0);
    if (rnd() % 3 == 0 && len > 24) { int sh = len - 25; if (sh > 0) { x &= ~((1ull << sh) - 1); if (rnd() & 1) x |= 1ull << (sh - 1); } }
    if (rnd() & 1) x = (uint64_t)0 - x; if (bits == 32) return (int32_t)(uint32_t)x; return (int64_t)x;
}
static const char *RMN[5] = {"RC=nearest", "RC=down", "RC=up", "RC=zero", "RC=any"};
static void mode_name(char *d, size_t n, int rcm, uint32_t mx, int dazonly, int sampled) {
    snprintf(d, n, "%s%s%s%s%s", RMN[rcm], (mx & 0x8000) ? "+FTZ" : "", (mx & 0x40) ? "+DAZ" : "", dazonly ? "+denormal_inputs_only" : "", sampled ? "+sampled" : "");
}
// float source class for f2i checks: 0 zero 1 denormal 2 in-range 3 overflow 4 inf 5 nan
static int f2i_cls(Fmt f, uint64_t x, int ind) { int c = fclass(f, x); if (c == 4) return 5; if (c == 3) return 4; if (c == 2 && ind) return 3; return c; }

// float/double -> integer. lanes = source elements per kernel iteration; pd2dq = the packed double->dword forms (4 dwords out per iteration)
typedef struct { const char *name; K k; int dbl, lanes, ibits, trunc, pd2dq; } F2I;
static int pass_f2i(const F2I *f, int rcm, uint32_t mx, int exhaustive, long long nsamp, int dazonly) {
    Fmt fm = f->dbl ? F64 : F32; int daz = !!(mx & 0x40), rm = f->trunc ? 4 : ((mx >> 13) & 3); const long long CH = 1 << 14;
    uint8_t *in = _aligned_malloc(CH * 8, 32), *out = _aligned_malloc(CH * 8 + 64, 32);
    Rep r; rep_init(&r, "zero,denormal,in_range,overflow,inf,nan,upper_lanes"); long long done = 0; uint64_t cur = 0; long long total = (exhaustive ? (dazonly ? (1ll << 24) : (1ll << 32)) : nsamp) >> g_fast; if (total < CH) total = CH;
    while (done < total) {
        for (long long i = 0; i < CH; i++) {
            uint64_t x;
            if (exhaustive) { uint64_t c = cur++; x = dazonly ? ((c & 0x7FFFFF) | ((c >> 23) << 31)) : c; } else x = f->dbl ? g64() : g32();
            if (f->dbl) ((uint64_t *)in)[i] = x; else ((uint32_t *)in)[i] = (uint32_t)x;
        }
        runk(f->k, in, out, CH / f->lanes, mx); done += CH;
        for (long long i = 0; i < CH; i++) {
            uint64_t x = f->dbl ? ((uint64_t *)in)[i] : ((uint32_t *)in)[i], xe = daz ? sf_daz(fm, x) : x, want, got;
            int ind = sf_f2i(fm, xe, rm, f->ibits, &want);
            if (g_neg && ind) { int c0 = fclass(fm, xe); uint64_t mx_ = f->ibits == 32 ? 0x7FFFFFFFull : 0x7FFFFFFFFFFFFFFFull; want = c0 == 4 ? 0 : ((xe >> (fm.eb + fm.mb)) & 1 ? (uint64_t)1 << (f->ibits - 1) : mx_); }
            if (f->pd2dq) {
                long long it = i / f->lanes, l = i % f->lanes; got = ((uint32_t *)out)[it * 4 + l];
                if (l == 0 && f->lanes == 2) { uint32_t u2 = ((uint32_t *)out)[it * 4 + 2], u3 = ((uint32_t *)out)[it * 4 + 3]; NOTE(&r, 6, u2 | u3, "x=%016llx upper dwords %08x %08x (must be zero)", (unsigned long long)x, u2, u3); }
            } else if (f->ibits == 32) got = ((uint32_t *)out)[i]; else got = ((uint64_t *)out)[i];
            int k = f2i_cls(fm, xe, ind); if (daz && fclass(fm, x) == 1) k = 1;
            NOTE(&r, k, got != want, "x=%016llx got=%llx want=%llx", (unsigned long long)x, (unsigned long long)got, (unsigned long long)want);
        }
    }
    char mode[80]; mode_name(mode, sizeof mode, rcm, mx, dazonly, !exhaustive); if (f->trunc) { char t2[96]; snprintf(t2, sizeof t2, "trunc@%s", mode); strcpy(mode, t2); }
    int fl = rep_emit("cvt", f->name, mode, &r, f->trunc ? "kind=truncating" : "kind=rounding_per_RC");
    _aligned_free(in); _aligned_free(out); return fl;
}

// integer -> float/double. kind 0: packed dq2ps (lanes 4/8), 1: scalar si2ss (lane 0, lanes 1..3 from g_pat), 2: scalar si2sd (lane 0 double, upper from g_pat[2..3]),
// 3: packed dq2pd (exact; lanes = ints per kernel iteration: 4 or 2)
typedef struct { const char *name; K k; int kind, ibits, lanes; } I2F;
static int pass_i2f(const I2F *f, int rcm, uint32_t mx, int exhaustive, long long nsamp) {
    int rm = (mx >> 13) & 3; const long long CH = 1 << 14; int ib = f->ibits / 8; Fmt to = (f->kind == 2 || f->kind == 3) ? F64 : F32;
    uint8_t *in = _aligned_malloc(CH * 8, 32), *out = _aligned_malloc(CH * 32 + 64, 32);
    Rep r; rep_init(&r, "zero,exact,rounded,upper_lanes"); long long done = 0; uint64_t cur = 0; long long total = (exhaustive ? (1ll << f->ibits) : nsamp) >> g_fast; if (total < CH) total = CH;
    int perout = f->kind == 0 ? 4 : (f->kind == 3 ? 8 : 16);   // output bytes per element... for scalar kinds the kernel writes 16 bytes per element
    while (done < total) {
        for (long long i = 0; i < CH; i++) {
            int64_t v = exhaustive ? (int64_t)(int32_t)(uint32_t)cur++ : gint(f->ibits);
            if (ib == 4) ((int32_t *)in)[i] = (int32_t)v; else ((int64_t *)in)[i] = v;
        }
        long long iters = (f->kind == 1 || f->kind == 2) ? CH : CH / f->lanes; runk(f->k, in, out, iters, mx); done += CH;
        for (long long i = 0; i < CH; i++) {
            int64_t v = ib == 4 ? ((int32_t *)in)[i] : ((int64_t *)in)[i]; uint64_t want = sf_i2f(to, v, f->kind == 3 ? 0 : rm), got;
            uint64_t mag = v < 0 ? (uint64_t)0 - (uint64_t)v : (uint64_t)v;
            int k = v == 0 ? 0 : ((to.mb == 23 ? mag < (1ull << 24) : mag < (1ull << 53)) ? 1 : 2);
            if (f->kind == 0) got = ((uint32_t *)out)[i];
            else if (f->kind == 3) got = ((uint64_t *)out)[i];
            else if (f->kind == 1) { got = ((uint32_t *)out)[i * 4]; NOTE(&r, 3, ((uint32_t *)out)[i * 4 + 1] != g_pat[1] || ((uint32_t *)out)[i * 4 + 2] != g_pat[2] || ((uint32_t *)out)[i * 4 + 3] != g_pat[3], "v=%lld upper lanes changed", (long long)v); }
            else { got = ((uint64_t *)out)[i * 2]; NOTE(&r, 3, ((uint32_t *)out)[i * 4 + 2] != g_pat[2] || ((uint32_t *)out)[i * 4 + 3] != g_pat[3], "v=%lld upper lanes changed", (long long)v); }
            if (f->kind == 0 || f->kind == 3) { /* result element lane i */ }
            NOTE(&r, k, got != want, "v=%lld got=%llx want=%llx", (long long)v, (unsigned long long)got, (unsigned long long)want);
        }
    }
    (void)perout; char mode[80]; mode_name(mode, sizeof mode, f->kind == 3 ? 0 : rcm, mx, 0, !exhaustive);
    int fl = rep_emit("cvt", f->name, mode, &r, "kind=int_to_float");
    _aligned_free(in); _aligned_free(out); return fl;
}

// float <-> double. kind 0: ps2pd packed (lanes 4 or 2), 1: ss2sd scalar (merge from g_pat[2..3]), 2: pd2ps packed (lanes 2 or 4; out has 4 floats, zero above), 3: sd2ss scalar (merge g_pat[1..3])
typedef struct { const char *name; K k; int kind, lanes; } F2F;
static int pass_f2f(const F2F *f, int rcm, uint32_t mx, int exhaustive, long long nsamp, int dazonly) {
    int rm = (mx >> 13) & 3, daz = !!(mx & 0x40), ftz = !!(mx & 0x8000); const long long CH = 1 << 14; int fromd = f->kind >= 2; Fmt from = fromd ? F64 : F32, to = fromd ? F32 : F64;
    uint8_t *in = _aligned_malloc(CH * 8, 32), *out = _aligned_malloc(CH * 32 + 64, 32);
    Rep r; rep_init(&r, "zero,denormal,normal,inf,nan,overflow,tiny,upper_lanes"); long long done = 0; uint64_t cur = 0; long long total = (exhaustive ? (dazonly ? (1ll << 24) : (1ll << 32)) : nsamp) >> g_fast; if (total < CH) total = CH;
    while (done < total) {
        for (long long i = 0; i < CH; i++) {
            uint64_t x; if (exhaustive) { uint64_t c = cur++; x = dazonly ? ((c & 0x7FFFFF) | ((c >> 23) << 31)) : c; } else x = fromd ? g64() : g32();
            if (fromd) ((uint64_t *)in)[i] = x; else ((uint32_t *)in)[i] = (uint32_t)x;
        }
        long long iters = (f->kind == 1 || f->kind == 3) ? CH : CH / f->lanes; runk(f->k, in, out, iters, mx); done += CH;
        for (long long i = 0; i < CH; i++) {
            uint64_t x = fromd ? ((uint64_t *)in)[i] : ((uint32_t *)in)[i], want = sf_f2f(from, to, x, rm, daz, ftz), got; int k = fclass(from, daz ? sf_daz(from, x) : x);
            if (daz && fclass(from, x) == 1) k = 1;
            if (f->kind == 0) got = ((uint64_t *)out)[i];
            else if (f->kind == 1) { got = ((uint64_t *)out)[i * 2]; NOTE(&r, 7, ((uint32_t *)out)[i * 4 + 2] != g_pat[2] || ((uint32_t *)out)[i * 4 + 3] != g_pat[3], "x=%llx upper lanes changed", (unsigned long long)x); }
            else if (f->kind == 2) {
                long long it = i / f->lanes, l = i % f->lanes; got = ((uint32_t *)out)[it * 4 + l];
                if (l == 0 && f->lanes == 2) NOTE(&r, 7, ((uint32_t *)out)[it * 4 + 2] | ((uint32_t *)out)[it * 4 + 3], "x=%llx upper floats not zero", (unsigned long long)x);
                if (k >= 2 && k <= 2) { Unp wu = sf_unpack(F32, want); if (wu.cls == 2) k = 5; else if (wu.cls == 0 || ((want >> 23) & 255) == 0) k = 6; }
            } else { got = ((uint32_t *)out)[i * 4]; NOTE(&r, 7, ((uint32_t *)out)[i * 4 + 1] != g_pat[1] || ((uint32_t *)out)[i * 4 + 2] != g_pat[2] || ((uint32_t *)out)[i * 4 + 3] != g_pat[3], "x=%llx upper lanes changed", (unsigned long long)x);
                if (k == 2) { Unp wu = sf_unpack(F32, want); if (wu.cls == 2) k = 5; else if (wu.cls == 0 || ((want >> 23) & 255) == 0) k = 6; } }
            int quiet_ok = 1; (void)quiet_ok;
            NOTE(&r, k, got != want, "x=%016llx got=%llx want=%llx", (unsigned long long)x, (unsigned long long)got, (unsigned long long)want);
        }
    }
    char mode[80]; mode_name(mode, sizeof mode, (f->kind >= 2) ? rcm : 0, mx, dazonly, !exhaustive);
    int fl = rep_emit("cvt", f->name, mode, &r, "kind=float_double");
    _aligned_free(in); _aligned_free(out); return fl;
}

// ---------------- batch driver ----------------
static int sel_ok(const char *sel, const char *name) { if (!sel) return 1; size_t n = strlen(sel); if (sel[0] == '*') return strstr(name, sel + 1) != NULL; if (n && sel[n - 1] == '*') return !strncmp(name, sel, n - 1); return !strcmp(name, sel); }   // exact form name, *substring, or prefix*
static const uint32_t RCMX[4] = {0x1F80, 0x1F80 | (1 << 13), 0x1F80 | (2 << 13), 0x1F80 | (3 << 13)};
static const long long NS = 1ll << 26;
static int batch_cvt_f2i(const char *sel) {
    int fl = 0;
    static const F2I forms[] = {
        {"vcvttps2dq_ymm", k_tps2dq_y, 0, 8, 32, 1, 0}, {"vcvttps2dq_xmm", k_tps2dq_x, 0, 4, 32, 1, 0}, {"vcvtps2dq_ymm", k_ps2dq_y, 0, 8, 32, 0, 0}, {"vcvtps2dq_xmm", k_ps2dq_x, 0, 4, 32, 0, 0},
        {"vcvttss2si_r32", k_ttss2si32, 0, 1, 32, 1, 0}, {"vcvttss2si_r64", k_ttss2si64, 0, 1, 64, 1, 0}, {"vcvtss2si_r32", k_ss2si32, 0, 1, 32, 0, 0}, {"vcvtss2si_r64", k_ss2si64, 0, 1, 64, 0, 0},
        {"vcvttsd2si_r32", k_ttsd2si32, 1, 1, 32, 1, 0}, {"vcvttsd2si_r64", k_ttsd2si64, 1, 1, 64, 1, 0}, {"vcvtsd2si_r32", k_sd2si32, 1, 1, 32, 0, 0}, {"vcvtsd2si_r64", k_sd2si64, 1, 1, 64, 0, 0},
        {"vcvttpd2dq_xmm", k_ttpd2dq_x, 1, 2, 32, 1, 1}, {"vcvttpd2dq_ymm", k_ttpd2dq_y, 1, 4, 32, 1, 1}, {"vcvtpd2dq_xmm", k_pd2dq_x, 1, 2, 32, 0, 1}, {"vcvtpd2dq_ymm", k_pd2dq_y, 1, 4, 32, 0, 1},
        {"vcvttps2dq_ymm_mem", m_tps2dq_y, 0, 8, 32, 1, 0}, {"vcvttps2dq_xmm_mem", m_tps2dq_x, 0, 4, 32, 1, 0}, {"vcvtps2dq_ymm_mem", m_ps2dq_y, 0, 8, 32, 0, 0}, {"vcvtps2dq_xmm_mem", m_ps2dq_x, 0, 4, 32, 0, 0},
        {"vcvttss2si_r32_mem", m_ttss2si32, 0, 1, 32, 1, 0}, {"vcvttss2si_r64_mem", m_ttss2si64, 0, 1, 64, 1, 0}, {"vcvtss2si_r32_mem", m_ss2si32, 0, 1, 32, 0, 0}, {"vcvtss2si_r64_mem", m_ss2si64, 0, 1, 64, 0, 0},
        {"vcvttsd2si_r32_mem", m_ttsd2si32, 1, 1, 32, 1, 0}, {"vcvttsd2si_r64_mem", m_ttsd2si64, 1, 1, 64, 1, 0}, {"vcvtsd2si_r32_mem", m_sd2si32, 1, 1, 32, 0, 0}, {"vcvtsd2si_r64_mem", m_sd2si64, 1, 1, 64, 0, 0},
        {"vcvttpd2dq_xmm_mem", m_ttpd2dq_x, 1, 2, 32, 1, 1}, {"vcvttpd2dq_ymm_mem", m_ttpd2dq_y, 1, 4, 32, 1, 1}, {"vcvtpd2dq_xmm_mem", m_pd2dq_x, 1, 2, 32, 0, 1}, {"vcvtpd2dq_ymm_mem", m_pd2dq_y, 1, 4, 32, 0, 1}};
    for (unsigned i = 0; i < sizeof forms / sizeof *forms; i++) {
        const F2I *f = &forms[i]; if (!sel_ok(sel, f->name)) continue;
        int ex = !f->dbl && (f->lanes == 8 || f->lanes == 1) && !strstr(f->name, "_mem");      // exhaustive over all 2^32 float inputs for the ymm and scalar forms
        if (f->trunc) {
            if (ex) fl += pass_f2i(f, 0, RCMX[0], 1, 0, 0); else fl += pass_f2i(f, 0, RCMX[0], 0, NS * 2, 0);
            for (int rc = 1; rc < 4; rc++) fl += pass_f2i(f, rc, RCMX[rc], 0, NS, 0);      // truncation must not depend on RC
            fl += pass_f2i(f, 0, RCMX[0] | 0x40, 0, NS / 4, 0);
        } else {
            for (int rc = 0; rc < 4; rc++) { if (ex && !f->dbl) fl += pass_f2i(f, rc, RCMX[rc], 1, 0, 0); else fl += pass_f2i(f, rc, RCMX[rc], 0, NS * 2, 0); }
            for (int rc = 0; rc < 4; rc++) { if (!f->dbl) fl += pass_f2i(f, rc, RCMX[rc] | 0x40, 1, 0, 1); else fl += pass_f2i(f, rc, RCMX[rc] | 0x40, 0, NS / 2, 0); }
        }
    }
    return fl;
}
static int batch_cvt_i2f(const char *sel) {
    int fl = 0;
    static const I2F forms[] = {
        {"vcvtdq2ps_ymm", k_dq2ps_y, 0, 32, 8}, {"vcvtdq2ps_xmm", k_dq2ps_x, 0, 32, 4}, {"vcvtsi2ss_r32", k_si2ss32, 1, 32, 1}, {"vcvtsi2ss_r64", k_si2ss64, 1, 64, 1},
        {"vcvtsi2sd_r32", k_si2sd32, 2, 32, 1}, {"vcvtsi2sd_r64", k_si2sd64, 2, 64, 1}, {"vcvtdq2pd_ymm", k_dq2pd_y, 3, 32, 4}, {"vcvtdq2pd_xmm", k_dq2pd_x, 3, 32, 2},
        {"vcvtdq2ps_ymm_mem", m_dq2ps_y, 0, 32, 8}, {"vcvtdq2ps_xmm_mem", m_dq2ps_x, 0, 32, 4}, {"vcvtsi2ss_r32_mem", m_si2ss32, 1, 32, 1}, {"vcvtsi2ss_r64_mem", m_si2ss64, 1, 64, 1},
        {"vcvtsi2sd_r32_mem", m_si2sd32, 2, 32, 1}, {"vcvtsi2sd_r64_mem", m_si2sd64, 2, 64, 1}, {"vcvtdq2pd_ymm_mem", m_dq2pd_y, 3, 32, 4}, {"vcvtdq2pd_xmm_mem", m_dq2pd_x, 3, 32, 2}};
    for (unsigned i = 0; i < sizeof forms / sizeof *forms; i++) {
        const I2F *f = &forms[i]; if (!sel_ok(sel, f->name)) continue;
        int ex32 = !strstr(f->name, "_mem") && f->ibits == 32 && (f->lanes == 8 || f->lanes == 4 || f->kind == 1 || f->kind == 2) && !(f->kind == 0 && f->lanes == 4);
        if (f->kind == 3) { if (f->lanes == 4) fl += pass_i2f(f, 0, RCMX[0], 1, 0); else fl += pass_i2f(f, 0, RCMX[0], 0, NS); continue; }
        for (int rc = 0; rc < 4; rc++) {
            if (ex32 && (f->lanes == 8 || rc == 0)) fl += pass_i2f(f, rc, RCMX[rc], 1, 0);
            else fl += pass_i2f(f, rc, RCMX[rc], 0, NS * 2);
        }
    }
    return fl;
}
static int batch_cvt_f2f(const char *sel) {
    int fl = 0;
    static const F2F forms[] = {
        {"vcvtps2pd_ymm", k_ps2pd_y, 0, 4}, {"vcvtps2pd_xmm", k_ps2pd_x, 0, 2}, {"vcvtss2sd", k_ss2sd, 1, 1},
        {"vcvtpd2ps_xmm", k_pd2ps_x, 2, 2}, {"vcvtpd2ps_ymm", k_pd2ps_y, 2, 4}, {"vcvtsd2ss", k_sd2ss, 3, 1},
        {"vcvtps2pd_ymm_mem", m_ps2pd_y, 0, 4}, {"vcvtps2pd_xmm_mem", m_ps2pd_x, 0, 2}, {"vcvtss2sd_mem", m_ss2sd, 1, 1}, {"vcvtpd2ps_xmm_mem", m_pd2ps_x, 2, 2}, {"vcvtpd2ps_ymm_mem", m_pd2ps_y, 2, 4}, {"vcvtsd2ss_mem", m_sd2ss, 3, 1}};
    for (unsigned i = 0; i < sizeof forms / sizeof *forms; i++) {
        const F2F *f = &forms[i]; if (!sel_ok(sel, f->name)) continue;
        if (f->kind < 2) {
            int ex = f->lanes != 2 && !strstr(f->name, "_mem");
            if (ex) fl += pass_f2f(f, 0, RCMX[0], 1, 0, 0); else fl += pass_f2f(f, 0, RCMX[0], 0, NS * 2, 0);
            fl += pass_f2f(f, 0, RCMX[0] | 0x40, 1, 0, 1);                 // DAZ over every denormal float
            fl += pass_f2f(f, 2, RCMX[2] | 0x40 | 0x8000, 0, NS / 4, 0);   // RC up + DAZ + FTZ (exact conversion: none of them may matter except DAZ)
        } else {
            for (int rc = 0; rc < 4; rc++) fl += pass_f2f(f, rc, RCMX[rc], 0, NS * 2, 0);
            for (int rc = 0; rc < 4; rc++) { fl += pass_f2f(f, rc, RCMX[rc] | 0x8000, 0, NS, 0); fl += pass_f2f(f, rc, RCMX[rc] | 0x40, 0, NS / 2, 0); fl += pass_f2f(f, rc, RCMX[rc] | 0x8000 | 0x40, 0, NS / 2, 0); }
        }
    }
    return fl;
}
