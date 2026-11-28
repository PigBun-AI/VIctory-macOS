// Checks isa-model-int.h (flags of add/sub/adc/sbb/and/or/xor/cmp/test/inc/dec/neg) against arm64 hardware flags where Arm has them
// (N Z C V of adds/subs/adcs/sbcs/ands, widths 32/64 directly and 8/16 by shifting the operands to the top of a 32-bit register) and, for the flags Arm lacks
// (PF, AF), against a second, independent formulation (popcount of the low byte; carry out of bit 3):  clang -O1 -I.. validate-int.c -o v && ./v
#include "../isa-model-int.h"
static uint64_t s = 88172645463325252ull;
static uint64_t rnd(void) { s ^= s << 13; s ^= s >> 7; s ^= s << 17; return s; }
static uint64_t gen(int bits) { uint64_t m = mskb(bits), x = rnd() & m; switch (rnd() % 8) { case 0: x = 0; break; case 1: x = m; break; case 2: x = 1ull << (bits - 1); break; case 3: x = (1ull << (bits - 1)) - 1; break; case 4: x = 1; break; case 5: x = rnd() & 0xFF & m; break; default: break; } return x; }
int main(void) {
    long bad = 0, total = 0;
    for (long it = 0; it < 30000000; it++) {
        int wbs[4] = {1, 2, 4, 8}; int wb = wbs[it & 3], bits = wb * 8; uint64_t a = gen(bits), b = gen(bits), m = mskb(bits); uint32_t cf = (uint32_t)(rnd() & 1), fin = cf | (uint32_t)(rnd() & 0x8D4);
        for (int kind = IA_ADD; kind <= IA_NOT; kind++) {
            AluR r = alu(kind, wb, a, b, fin); uint64_t er = 0; uint32_t ef = 0; int hw = 0;
            // hardware: compute result and C/V/N/Z with arm64 flag instructions
            uint64_t ra = bits == 64 ? a : a << (64 - bits), rb = bits == 64 ? b : b << (64 - bits), res = 0, nzcv = 0;       // operands at the top of a 64-bit register: flags of the 64-bit op are the flags of the narrow op
            switch (kind) {
            case IA_ADD: __asm__("adds %1, %2, %3\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(ra), "r"(rb) : "cc"); hw = 1; break;
            case IA_SUB: case IA_CMP: __asm__("subs %1, %2, %3\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(ra), "r"(rb) : "cc"); hw = 1; break;
            case IA_ADC: { uint64_t one = cf; ra |= (1ull << (64 - bits)) - 1; __asm__("subs xzr, %4, #1\n adcs %1, %2, %3\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(ra), "r"(rb), "r"(one) : "cc"); hw = 1; break; }
            case IA_SBB: { uint64_t one = cf ^ 1; __asm__("subs xzr, %4, #1\n sbcs %1, %2, %3\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(ra), "r"(rb), "r"(one) : "cc"); hw = 1; break; }
            case IA_AND: case IA_TEST: __asm__("ands %1, %2, %3\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(ra), "r"(rb) : "cc"); hw = 2; break;
            case IA_OR: res = ra | rb; hw = 3; break; case IA_XOR: res = ra ^ rb; hw = 3; break;
            case IA_INC: __asm__("adds %1, %2, %3\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(ra), "r"((uint64_t)1 << (64 - bits)) : "cc"); hw = 4; break;
            case IA_DEC: __asm__("subs %1, %2, %3\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(ra), "r"((uint64_t)1 << (64 - bits)) : "cc"); hw = 5; break;
            case IA_NEG: __asm__("subs %1, xzr, %2\n mrs %0, nzcv" : "=r"(nzcv), "=r"(res) : "r"(ra) : "cc"); hw = 6; break;
            default: hw = 0; }
            total++;
            if (kind == IA_NOT) { if (r.r != (~a & m) || r.f != (fin & FL_ALL)) bad++; continue; }
            er = res >> (64 - bits); if (kind == IA_OR || kind == IA_XOR) { nzcv = (res == 0 ? 4ull << 28 : 0) | ((res >> 63) ? 8ull << 28 : 0); }
            uint32_t N = (uint32_t)((er >> (bits - 1)) & 1), Z = er == 0, C = (nzcv >> 29) & 1, V = (nzcv >> 28) & 1;
            uint32_t cfx = (kind == IA_SUB || kind == IA_SBB || kind == IA_CMP || kind == IA_DEC || kind == IA_NEG) ? !C : C;      // x86 CF = borrow
            if (kind == IA_AND || kind == IA_TEST || kind == IA_OR || kind == IA_XOR) cfx = 0;
            if (kind == IA_INC || kind == IA_DEC) cfx = fin & 1;
            uint32_t p = (uint32_t)(er & 0xFF); int pf = __builtin_popcount(p) % 2 == 0;
            // AF: carry/borrow out of bit 3, formulated on the nibbles
            uint32_t af = 0; { uint32_t x = (uint32_t)(a & 15), y = (uint32_t)(b & 15); int c = 0;
                switch (kind) { case IA_ADD: af = x + y > 15; break; case IA_ADC: af = x + y + (fin & 1) > 15; break; case IA_SUB: case IA_CMP: af = x < y; break; case IA_SBB: af = x < y + (fin & 1); break;
                case IA_INC: af = x + 1 > 15; break; case IA_DEC: af = x < 1; break; case IA_NEG: af = x != 0; break; default: af = 0; } (void)c; }
            ef = (cfx ? FL_CF : 0) | (pf ? FL_PF : 0) | (Z ? FL_ZF : 0) | (N ? FL_SF : 0) | (V ? FL_OF : 0) | (af ? FL_AF : 0);
            if (kind == IA_AND || kind == IA_TEST || kind == IA_OR || kind == IA_XOR) { ef &= ~(uint32_t)FL_OF; r.f &= ~(uint32_t)FL_AF; ef &= ~(uint32_t)FL_AF; }     // AF undefined for the logic ops
            if (kind == IA_NEG) { /* arm: subs xzr-a: V as for 0 - a */ }
            if (kind != IA_CMP && kind != IA_TEST && r.r != er) { if (bad++ < 10) printf("result kind=%d wb=%d a=%llx b=%llx got=%llx want=%llx\n", kind, wb, (unsigned long long)a, (unsigned long long)b, (unsigned long long)r.r, (unsigned long long)er); }
            else if (r.f != ef) { if (bad++ < 10) printf("flags kind=%d wb=%d a=%llx b=%llx fin=%x model=%x hw=%x\n", kind, wb, (unsigned long long)a, (unsigned long long)b, fin, r.f, ef); }
            (void)hw;
        }
    }
    printf("validate-int: %ld checks, %ld mismatches\n", total, bad); return bad != 0;
}
