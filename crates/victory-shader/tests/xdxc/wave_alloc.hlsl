// Our own HLSL (xdxc oracle): wave prefix sum + atomic allocation, 64 threads a group.
// Each lane owns c = in[tid] & 7 slots; one lane per wave takes the wave's total from a global counter, the lanes
// write their ids at base + exclusive prefix. The slot a lane gets depends on which wave won the counter and on the
// wave width, so the test compares the multiset of the records and the counter, not their order.
//   VARIANT 0: every lane takes part in the wave operations.
//   VARIANT 1: only lanes with c != 0 take part (divergent EXEC around the wave operations).
//   VARIANT 2: stream compaction by ballot: lanes with the low bit set, WavePrefixCountBits.
#ifndef VARIANT
#define VARIANT 0
#endif
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=2))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(recs, 1, 0)
DUAL_UAV(counter, 2, 1)

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(recs, 1, 0)
    DUAL_BIND_UAV(counter, 2, 1)
    uint v = src.Load(tid.x * 4);
#if VARIANT == 0
    uint c = v & 7;
    uint pre = WavePrefixSum(c);
    uint tot = WaveActiveSum(c);
    uint base = 0;
    if (WaveIsFirstLane())
        counter.InterlockedAdd(0, tot, base);
    base = WaveReadLaneFirst(base);
    for (uint i = 0; i < c; i++)
        recs.Store((base + pre + i) * 4, tid.x + 1);
#elif VARIANT == 1
    uint c = v & 7;
    if (c != 0)
    {
        uint pre = WavePrefixSum(c);
        uint tot = WaveActiveSum(c);
        uint base = 0;
        if (WaveIsFirstLane())
            counter.InterlockedAdd(0, tot, base);
        base = WaveReadLaneFirst(base);
        for (uint i = 0; i < c; i++)
            recs.Store((base + pre + i) * 4, tid.x + 1);
    }
#else
    bool keep = (v & 1) != 0;
    uint pre = WavePrefixCountBits(keep);
    uint tot = WaveActiveCountBits(keep);
    uint base = 0;
    if (WaveIsFirstLane())
        counter.InterlockedAdd(0, tot, base);
    base = WaveReadLaneFirst(base);
    if (keep)
        recs.Store((base + pre) * 4, tid.x + 1);
#endif
}
