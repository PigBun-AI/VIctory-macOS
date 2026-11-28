// Our own HLSL (xdxc oracle): atomics on groupshared memory, 64 threads a group. All 64 lanes hit g[0..7]; some ops only from odd lanes.
// out0 = the 8 groupshared words afterwards; out1, 192 dwords a group = the old values returned by add-1 (a permutation of 0..63), those returned by exchange, and the exchanged word's final value.
// The order the lanes win is not part of the result, so the test sorts what returns are.
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=2))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(fin, 1, 0)
DUAL_UAV(olds, 2, 1)
groupshared uint g[8];

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID, uint gi : SV_GroupIndex, uint3 gid : SV_GroupID)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(fin, 1, 0)
    DUAL_BIND_UAV(olds, 2, 1)
    uint v = src.Load(tid.x * 4);
    if (gi < 8)
    {
        uint init[8] = { 0, 0xFFFFFFFF, 0, 0, 0x12345678, 0xFFFFFFFF, 7, 100 };
        g[gi] = init[gi];
    }
    GroupMemoryBarrierWithGroupSync();
    uint o1, o2, o3;
    InterlockedAdd(g[0], 1, o1);
    InterlockedMin(g[1], v);
    InterlockedMax(g[2], v);
    InterlockedOr(g[3], 1u << (v & 31));
    InterlockedXor(g[4], v);
    if (gi & 1)
        InterlockedAnd(g[5], v | 0x0F0F0F0F);
    if (gi >= 32)
        InterlockedAdd(g[6], v & 0xFF);
    InterlockedExchange(g[7], gi + 1, o3);
    GroupMemoryBarrierWithGroupSync();
    olds.Store((gid.x * 192 + gi) * 4, o1);
    olds.Store((gid.x * 192 + 64 + gi) * 4, o3);
    if (gi == 0)
        olds.Store((gid.x * 192 + 128) * 4, g[7]);
    if (gi < 8)
        fin.Store((gid.x * 8 + gi) * 4, g[gi]);
}
