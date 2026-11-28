// Two independent prefix streams. All lanes reach both shuffles, including
// lanes whose input is outside the application count (their contribution is zero).
#if !DUAL_A
#define __XBOX_ENABLE_WAVE32 0
#endif
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=2))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(slots, 1, 0)
DUAL_UAV(counters, 2, 1)
DUAL_ROOT(RS)
[numthreads(64,1,1)]
void main(uint3 tid : SV_DispatchThreadID) {
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(slots, 1, 0)
    DUAL_BIND_UAV(counters, 2, 1)
    uint2 n = 0;
    if (tid.x < params.x) {
        uint v = src.Load(tid.x*4);
        n.x = v & 1;
        if (params.y) n.y = (v >> 2) & 1;
    }
    uint2 prefix = uint2(WavePrefixSum(n.x), WavePrefixSum(n.y));
    uint2 base = 0;
    uint tail = WaveGetLaneCount()-1;
    if (WaveGetLaneIndex() == tail) {
        uint2 total = prefix+n;
        uint ax = WaveActiveSum(total.x);
        if (WaveIsFirstLane()) counters.InterlockedAdd(0, ax, base.x);
        base.x = WaveReadLaneFirst(base.x) + WavePrefixSum(total.x);
        if (params.y) {
            uint ay = WaveActiveSum(total.y);
            if (WaveIsFirstLane()) counters.InterlockedAdd(4, ay, base.y);
            base.y = WaveReadLaneFirst(base.y) + WavePrefixSum(total.y);
        }
    }
    base.x = WaveReadLaneAt(base.x,tail);
    if (params.y) base.y = WaveReadLaneAt(base.y,tail);
    if (tid.x < params.x) {
        uint divisor = params.z;
        uint unused;
        if (n.x) {
            slots.Store(4*(base.x+prefix.x), tid.x+1);
            counters.InterlockedMax(8,(tid.x+divisor-1)/divisor,unused);
        }
        if (params.y) {
            if (n.y) {
                slots.Store(4*(256+base.y+prefix.y), tid.x+1);
                uint tag = src.Load(tid.x*4) >> 8;
                // The oracle seeds the same tag for each contributing thread;
                // a compiler cannot assume that this data-dependent branch is uniform.
                uint4 votes = WaveActiveBallot(tag != 0);
                [branch] if (countbits(votes.x) + countbits(votes.y) > params.w) {
                    uint leaderTag = WaveActiveMax(tag);
                    counters.InterlockedMax(12,(2*tid.x+divisor-1)/divisor + leaderTag,unused);
                }
            }
        }
    }
}
