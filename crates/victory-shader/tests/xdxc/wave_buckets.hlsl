// Our own HLSL (xdxc oracle): wave-wide aggregated atomics over a waterfall loop, 64 threads a group. Lanes group themselves by a key
// (v % KEYS); one lane of each group takes the group's count from a global counter (add) and publishes the group's maximum (umax); every
// lane writes (key, base + its rank among the lanes of its key). Which wave gets which base is the atomic's race, so the test compares
// the sorted (key, slot) pairs and the two counter arrays. Lane order and wave width do not enter the result.
//   VARIANT 0: the waterfall loop: WaveReadLaneFirst(key), the lanes that match take the turn, the rest loop.
//   VARIANT 2: as 0, but only lanes with (v & 3) != 0 take part (divergent entry); the others write slot 0xFFFFFFFF.
#ifndef VARIANT
#define VARIANT 0
#endif
#ifndef KEYS
#define KEYS 16
#endif
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=2))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(slots, 1, 0)
DUAL_UAV(counts, 2, 1)

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(slots, 1, 0)
    DUAL_BIND_UAV(counts, 2, 1)
    if (tid.x >= params.x || (tid.x & 63) < params.z)
        return; // lanes past the count, or the first params.z lanes of each group, leave early: a partial wave, or a whole host wave gone
    uint v = src.Load(tid.x * 4);
    uint key = v % KEYS;
    uint slot = 0xFFFFFFFF;
    bool done = false;
#if VARIANT == 2
    done = (v & 3) == 0;
#endif
    [loop] while (WaveActiveAnyTrue(!done))
    {
        if (!done)
        {
            uint k0 = WaveReadLaneFirst(key);
            if (key == k0)
            {
                uint cnt = WaveActiveCountBits(true);
                uint rank = WavePrefixCountBits(true);
                uint mx = WaveActiveMax(v);
                uint base = 0;
                if (WaveIsFirstLane())
                {
                    counts.InterlockedAdd(k0 * 4, cnt, base);
                    counts.InterlockedMax((16 + k0) * 4, mx);
                }
                base = WaveReadLaneFirst(base);
                slot = base + rank;
                done = true;
            }
        }
    }
    slots.Store((tid.x * 2) * 4, key);
    slots.Store((tid.x * 2 + 1) * 4, slot);
}
