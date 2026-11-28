// Our own HLSL (xdxc oracle, S3): divergent loops and group barriers, 64 threads a group (one console wave64).
//   VARIANT 0: a loop whose trip count differs per lane, then three phases through groupshared memory with barriers
//              after the loop (the barrier is reached by every lane, the loop exits are divergent).
//   VARIANT 1: a barrier inside a loop of uniform trip count whose body is divergent (a lane writes or not).
#ifndef VARIANT
#define VARIANT 0
#endif
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=1))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(dst, 1, 0)
groupshared uint lds[64];

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID, uint gi : SV_GroupIndex)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(dst, 1, 0)
    uint v = src.Load(tid.x * 4);
#if VARIANT == 0
    uint n = v & 15;
    uint acc = gi + params.x;
    for (uint i = 0; i < n; i++)
    {
        acc = acc * 3 + i;
        if (acc & 1)
            acc ^= v;
    }
    lds[gi] = acc;
    GroupMemoryBarrierWithGroupSync();
    uint nb = lds[(gi + 1) & 63] ^ lds[(gi + 37) & 63];
    GroupMemoryBarrierWithGroupSync();
    lds[gi] = nb + acc;
    GroupMemoryBarrierWithGroupSync();
    dst.Store(tid.x * 4, lds[63 - gi] ^ acc);
#else
    uint acc = v;
    lds[gi] = 0;
    GroupMemoryBarrierWithGroupSync();
    for (uint r = 0; r < 4; r++)
    {
        if (((v >> r) & 1) != 0)
            lds[gi] += acc + r;
        GroupMemoryBarrierWithGroupSync();
        acc += lds[(gi + r + 1) & 63];
        GroupMemoryBarrierWithGroupSync();
    }
    dst.Store(tid.x * 4, acc ^ lds[gi]);
#endif
}
