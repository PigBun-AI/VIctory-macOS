// Our own HLSL (xdxc oracle): wave operations and an atomic inside a branch that only some lanes of a 64-lane wave take. Only the
// console path runs it (the result is for a 64-lane wave) and the result is compared with a CPU model. 64 threads a group, one wave.
// COND picks the lanes that take the branch:
//   0: the last lane   1: lane 40   2: lanes 32..63 (the second host wave of a console wave)   3: lanes 0..31   4: even lanes   5: lanes 5 and 60
// Inside: r = WaveActiveSum(a), p = WavePrefixSum(a), q = WaveActiveMax(a); the first active lane adds r + 1 to a counter and keeps what
// it got back in bases[wave]. After: every lane reads r from lane X (an active lane) with WaveReadLaneAt.
//   out[t*4+0] = r (or 0xFFFFFFFF outside) ; +1 = p ; +2 = q ; +3 = r read from lane X by every lane
#ifndef COND
#define COND 0
#endif
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=3))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(dst, 1, 0)
DUAL_UAV(bases, 2, 1)
DUAL_UAV(counter, 3, 2)

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID, uint3 gid : SV_GroupID)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(dst, 1, 0)
    DUAL_BIND_UAV(bases, 2, 1)
    DUAL_BIND_UAV(counter, 3, 2)
    uint a = src.Load(tid.x * 4) & 255;
    uint lane = WaveGetLaneIndex();
    uint last = WaveGetLaneCount() - 1;
#if COND == 0
    bool take = lane == last;
    uint X = 63;
#elif COND == 1
    bool take = lane == 40;
    uint X = 40;
#elif COND == 2
    bool take = lane >= 32;
    uint X = 32;
#elif COND == 3
    bool take = lane < 32;
    uint X = 0;
#elif COND == 4
    bool take = (lane & 1) == 0;
    uint X = 0;
#else
    bool take = lane == 5 || lane == 60;
    uint X = 5;
#endif
    uint r = 0xFFFFFFFF, p = 0xFFFFFFFF, q = 0xFFFFFFFF;
    if (take)
    {
        r = WaveActiveSum(a);
        p = WavePrefixSum(a);
        q = WaveActiveMax(a);
        if (WaveIsFirstLane())
        {
            uint base;
            counter.InterlockedAdd(0, r + 1, base);
            bases.Store(gid.x * 4, base);
        }
    }
    uint rr = WaveReadLaneAt(r, X);
    dst.Store((tid.x * 4) * 4, r);
    dst.Store((tid.x * 4 + 1) * 4, p);
    dst.Store((tid.x * 4 + 2) * 4, q);
    dst.Store((tid.x * 4 + 3) * 4, rr);
}
