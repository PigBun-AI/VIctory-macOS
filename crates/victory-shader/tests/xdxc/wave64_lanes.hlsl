// Our own HLSL (xdxc oracle): wave operations whose result depends on the wave being 64 lanes wide (the console's wave size), so
// only the console path runs it and the result is compared with a CPU model of a 64-lane wave. 64 threads a group, one group per wave.
//   out[t*8+0] = WaveReadLaneAt(v, 63)         out[t*8+1] = WaveReadLaneAt(v, 40)
//   out[t*8+2] = lanes with v bit0 (ballot popcount, 64 lanes)          out[t*8+3] = WavePrefixSum(v & 255)
//   out[t*8+4] = WaveActiveSum(v & 255)         out[t*8+5] = inside `if (v & 2)`: WaveActiveSum(v & 255), else 0xFFFFFFFF
//   out[t*8+6] = inside `if (v & 2)`: WavePrefixSum(v & 255), else 0xFFFFFFFF
//   out[t*8+7] = WaveGetLaneIndex() + 100 * WaveGetLaneCount()
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=1))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(dst, 1, 0)

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(dst, 1, 0)
    uint v = src.Load(tid.x * 4);
    uint4 b = WaveActiveBallot((v & 1) != 0);
    uint o = tid.x * 8 * 4;
    dst.Store(o, WaveReadLaneAt(v, 63));
    dst.Store(o + 4, WaveReadLaneAt(v, 40));
    dst.Store(o + 8, countbits(b.x) + countbits(b.y));
    dst.Store(o + 12, WavePrefixSum(v & 255));
    dst.Store(o + 16, WaveActiveSum(v & 255));
    uint s = 0xFFFFFFFF, p = 0xFFFFFFFF;
    if (v & 2)
    {
        s = WaveActiveSum(v & 255);
        p = WavePrefixSum(v & 255);
    }
    dst.Store(o + 20, s);
    dst.Store(o + 24, p);
    dst.Store(o + 28, WaveGetLaneIndex() + 100 * WaveGetLaneCount());
}
