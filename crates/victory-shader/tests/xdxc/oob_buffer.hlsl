#ifndef STRADDLE
#define STRADDLE 0
#endif
// Our own HLSL (xdxc oracle): accesses past the end of a view. The SRV holds 16 dwords, the first UAV 40, the third 8. Every thread reads
// Load(tid*4), Load2 and Load4 at its own offset (the vector ones straddle the end), a read at an offset far past the end and one at
// 0xFFFFFFF0 (wraps); writes at tid*4 (threads 40..63 are out of range), a Store4 whose last two copies are past the end of the 8-dword UAV (the value depends only on the address), and an atomic add past the end.
// With STRADDLE the vector loads start at every dword, so some straddle the end; without it they are aligned to their size.
// HLSL says: reads out of range give 0, writes and atomics out of range are dropped.
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=3))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(dst, 1, 0)
DUAL_UAV(got, 2, 1)
DUAL_UAV(d4, 3, 2)

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(dst, 1, 0)
    DUAL_BIND_UAV(got, 2, 1)
    DUAL_BIND_UAV(d4, 3, 2)
    uint o = tid.x * 4;
    uint a = src.Load(o);
#if STRADDLE
    uint2 b = src.Load2(o);
    uint4 c = src.Load4(o);
#else
    uint2 b = src.Load2(tid.x * 8); // whole vectors: wholly inside or wholly outside
    uint4 c = src.Load4(tid.x * 16);
#endif
    uint far = src.Load(params.x + o);
    uint wrap = src.Load(0xFFFFFFF0 + o);
    uint k = tid.x * 8;
    got.Store(k * 4, a + 3 * b.y);
    got.Store((k + 1) * 4, c.x ^ c.y ^ c.z ^ c.w);
    got.Store((k + 2) * 4, far);
    got.Store((k + 3) * 4, wrap);
    got.Store((k + 4) * 4, c.w);
    got.Store((k + 5) * 4, b.x);
    got.Store((k + 6) * 4, c.z);
    got.Store((k + 7) * 4, a);
    dst.Store(o, tid.x + 1);
    d4.Store4((tid.x & 3) * 16, uint4(1, 2, 3, 4) + (tid.x & 3));
    uint old;
    dst.InterlockedAdd(o + 4096, 5, old);
}
