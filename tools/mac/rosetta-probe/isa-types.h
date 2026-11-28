// Portable types and helpers of the differential engine (no asm, no Windows): shared by the probe (isa-diff.h) and the native arm64
// validators of the software models (tests/validate-*.c).
#ifndef ISA_TYPES_H
#define ISA_TYPES_H
#include <stdint.h>
#include <string.h>
#include <stdio.h>
#include <stdlib.h>
typedef struct { _Alignas(32) uint8_t d[32], a[32], b[32], c[32]; } IRec;     // d: initial dst, a: src1 (VEX.vvvv), b: src2 (modrm), c: src3 (imm8[7:4]) ; memory source is the b field (+off)
typedef struct { _Alignas(32) uint8_t r[32]; uint64_t x[4]; } Out;           // r: destination register (or memory image, first 32 bytes), x[0..3]: GPR result / flags / memory image tail
typedef void (*KF)(const IRec *, Out *, long);
struct Op;
typedef struct { const struct Op *op; int w; int imm; uint32_t mx; } Ctx;
typedef void (*MF)(const Ctx *, const IRec *, Out *);
enum { EK_I8, EK_I16, EK_I32, EK_I64, EK_F32, EK_F64 };
enum { AL_DD, AL_DA, AL_DB, AL_AB, AL_ALL, AL_MEM };
typedef struct Op {
    const char *name; KF k; MF m; const void *p;
    int w, ek, imm, alias, memoff, cr;       // w: vector bytes (16/32); cr: bytes of r compared (default 32, 0 = r not written); memoff: offset of the memory source/destination
    uint64_t xm[4];                          // compared bits of x[0..3]
    int exh;                                 // 1: exhaustive pairs of elements (8-bit: all pairs in all lane positions; 16-bit: all 2^32 pairs)
    int mem_image;                           // 1: destination is memory: r||x is a 64-byte image pre-filled with d||c, compared in full
    int nsrc;                                // sources the model reads (for the generator): 1, 2 or 3
    int flt;                                 // floating point op: MXCSR variants apply
    int sz;                                  // bytes moved (loads/stores of less than a vector)
    int p2;                                  // store source offset inside the src1 field (vmovhps m64,x stores the high half)
    int al;                                  // required alignment of the memory operand (0 = none)
    int mem_r;                               // 1: destination is memory inside out.r (pre-filled with d); r is compared in full, x[] only where xm says
    const char *kimm;                        // immediate-stub table (STUB): the kernel for immediate I is kimm + 128 * I; the engine cycles I over the chunks of records
    int immn;                                // number of immediate values to cycle through (default 256)
    void (*umask)(const IRec *, const struct Op *, uint64_t m[4]);   // bits of x[0..3] to ignore for this record (architecturally undefined results / flags)
    void (*fix)(IRec *, const struct Op *, int stream);   // generator hook applied to every record (integer ops: flags, width-specific edge values)
} Op;
static const char *g_fam = "diff";
static const int ESZ[6] = {1, 2, 4, 8, 4, 8};


static inline uint64_t ld_(const uint8_t *p, int n) { uint64_t v = 0; memcpy(&v, p, n); return v; }
static inline void st_(uint8_t *p, int n, uint64_t v) { memcpy(p, &v, n); }
static inline uint64_t mskb(int bits) { return bits >= 64 ? ~0ull : ((1ull << bits) - 1); }
static inline int64_t sx_(uint64_t v, int bits) { return bits >= 64 ? (int64_t)v : (int64_t)(v << (64 - bits)) >> (64 - bits); }
static inline const uint8_t *msrc(const IRec *r, const Op *o) { return (const uint8_t *)r + 64 + o->memoff; }     // memory source bytes (the b field and what follows it in the record)

#endif
