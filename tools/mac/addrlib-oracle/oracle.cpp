// Oracle for victory-formats' texture layouts: AMD's addrlib (Mesa src/amd/addrlib, MIT) computes the
// surface size, the first mip of the tail and the address of every element; this prints them as the
// golden tables of crates/victory-formats/src/texture.rs (see build.sh and README.md).
//
// Input (stdin), one case per line:
//   <mode> <dim: 1|2|3> <element bytes> <block: 1|4> <width> <height> <depth-or-slices> <mips>
// Output, one line per case:
//   <size> <first tail mip> <fnv1a hash of the addresses> <number of elements> [<mip,x,y,z,address> samples...]
// The hash walks mips in order, then slices, rows, columns, 8 little-endian bytes per address.
// Settings: Navi 21 (GFX10.3), no pipe-bank XOR (pipeBankXor = 0); only the modes without XOR are asked.
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>
#include <string>
#include "addrinterface.h"
#include "amdgpu_asic_addr.h"
#define CIASICIDGFXENGINE_ARCTICISLAND 0x0000000D

static void *ADDR_API alloc(const ADDR_ALLOCSYSMEM_INPUT *in) { return malloc(in->sizeInBytes); }
static ADDR_E_RETURNCODE ADDR_API freeMem(const ADDR_FREESYSMEM_INPUT *in) { free(in->pVirtAddr); return ADDR_OK; }

static AddrFormat format_of(unsigned bpe, unsigned block) {
  if (block == 4) return bpe == 8 ? ADDR_FMT_BC1 : ADDR_FMT_BC7;
  switch (bpe) {
    case 1: return ADDR_FMT_8;
    case 2: return ADDR_FMT_16;
    case 4: return ADDR_FMT_8_8_8_8;
    case 8: return ADDR_FMT_16_16_16_16;
    default: return ADDR_FMT_32_32_32_32;
  }
}

int main(int argc, char **argv) {
  ADDR_CREATE_INPUT ci = {};
  ADDR_CREATE_OUTPUT co = {};
  ci.size = sizeof ci; co.size = sizeof co;
  ci.chipEngine = CIASICIDGFXENGINE_ARCTICISLAND;
  ci.chipFamily = FAMILY_NV;
  ci.chipRevision = 0x28;                      // NAVI21 (amdgpu_asic_addr.h)
  ci.callbacks.allocSysMem = alloc; ci.callbacks.freeSysMem = freeMem;
  ci.createFlags.value = 0;
  ci.createFlags.nonPower2MemConfig = 0;
  // GB_ADDR_CONFIG as a Navi 21 reports it: 16 pipes, 256-byte pipe interleave, 2 packers.
  // (Only the modes with XOR depend on it.)
  ci.regValue.gbAddrConfig = 4u | (0u << 3) | (3u << 6) | (2u << 8);   // NUM_PIPES 16, 256 B interleave, 4 packers
  if (AddrCreate(&ci, &co) != ADDR_OK) { fprintf(stderr, "AddrCreate failed\n"); return 2; }
  char line[512];
  while (fgets(line, sizeof line, stdin)) {
    unsigned mode, dim, bpe, block, w, h, d, mips;
    int used = 0;
    if (sscanf(line, "%u %u %u %u %u %u %u %u%n", &mode, &dim, &bpe, &block, &w, &h, &d, &mips, &used) != 8) continue;
    std::vector<unsigned> probes;   // optional: "@ mip x y z ..." addresses to print too
    if (const char *at = strchr(line + used, '@')) {
      const char *p = at + 1; int n = 0; unsigned v;
      while (sscanf(p, "%u%n", &v, &n) == 1) { probes.push_back(v); p += n; }
    }
    ADDR2_COMPUTE_SURFACE_INFO_INPUT in = {};
    ADDR2_COMPUTE_SURFACE_INFO_OUTPUT out = {};
    ADDR2_MIP_INFO mi[16] = {};
    in.size = sizeof in; out.size = sizeof out; out.pMipInfo = mi;
    in.swizzleMode = (AddrSwizzleMode)mode;
    in.resourceType = dim == 1 ? ADDR_RSRC_TEX_1D : dim == 3 ? ADDR_RSRC_TEX_3D : ADDR_RSRC_TEX_2D;
    in.format = format_of(bpe, block);
    in.bpp = bpe * 8;
    in.width = w; in.height = h; in.numSlices = d; in.numMipLevels = mips; in.numSamples = 1;
    if (Addr2ComputeSurfaceInfo(co.hLib, &in, &out) != ADDR_OK) { printf("ERR info\n"); continue; }
    // firstMipIdInTail is the number of mips when none is in the tail; a linear surface has no tail.
    unsigned first_tail = mode == ADDR_SW_LINEAR ? mips : out.firstMipIdInTail;
    unsigned long long h64 = 0xcbf29ce484222325ull, count = 0;
    // XOR modes (>= 16): the size and the tail only; their addresses depend on the pipe-bank XOR of each surface.
    if (mode >= 16) { printf("%llu %u 0x0 0\n", (unsigned long long)out.surfSize, first_tail); continue; }
    std::string samples;
    for (unsigned m = 0; m < mips; m++) {
      unsigned mw = (w >> m) ? (w >> m) : 1, mh = (h >> m) ? (h >> m) : 1, md = (d >> m) ? (d >> m) : 1;
      unsigned ew = (mw + block - 1) / block, eh = (mh + block - 1) / block;
      unsigned slices = dim == 3 ? md : d;
      for (unsigned z = 0; z < slices; z++)
        for (unsigned y = 0; y < eh; y++)
          for (unsigned x = 0; x < ew; x++) {
            ADDR2_COMPUTE_SURFACE_ADDRFROMCOORD_INPUT ai = {};
            ADDR2_COMPUTE_SURFACE_ADDRFROMCOORD_OUTPUT ao = {};
            ai.size = sizeof ai; ao.size = sizeof ao;
            ai.x = x; ai.y = y; ai.slice = z; ai.sample = 0; ai.mipId = m;
            ai.swizzleMode = in.swizzleMode; ai.resourceType = in.resourceType;
            ai.bpp = in.bpp;
            ai.unalignedWidth = (w + block - 1) / block; ai.unalignedHeight = (h + block - 1) / block;
            ai.numSlices = d; ai.numMipLevels = mips; ai.numSamples = 1; ai.numFrags = 1;
            ai.pipeBankXor = 0;
            ai.pitchInElement = out.pitch;
            if (Addr2ComputeSurfaceAddrFromCoord(co.hLib, &ai, &ao) != ADDR_OK) { printf("ERR coord %u %u %u %u\n", m, x, y, z); goto next; }
            for (size_t k = 0; k + 3 < probes.size(); k += 4)
              if (probes[k] == m && probes[k + 1] == x && probes[k + 2] == y && probes[k + 3] == z)
                samples += " " + std::to_string((unsigned long long)ao.addr);
            for (int b = 0; b < 8; b++) { h64 ^= (ao.addr >> (8 * b)) & 0xff; h64 *= 0x100000001b3ull; }
            count++;
          }
    }
    printf("%llu %u 0x%016llx %llu%s\n", (unsigned long long)out.surfSize, first_tail, h64, count, samples.c_str());
  next:;
  }
  return 0;
}
