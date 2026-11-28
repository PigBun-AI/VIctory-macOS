// Our own HLSL (xdxc oracle, S7): the 16-bit conversions. out0[i] = f32tof16(asfloat(in0[i])) (the bits of a half in the low
// half), out1[i] = asuint(f16tof32(in1[i] & 0xFFFF)). One thread per element, groups of 64.
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=2), UAV(u0, numDescriptors=2))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(in0, 0, 0)
DUAL_SRV(in1, 1, 1)
DUAL_UAV(out0, 2, 0)
DUAL_UAV(out1, 3, 1)

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    DUAL_BIND_SRV(in0, 0, 0)
    DUAL_BIND_SRV(in1, 1, 1)
    DUAL_BIND_UAV(out0, 2, 0)
    DUAL_BIND_UAV(out1, 3, 1)
    out0.Store(tid.x * 4, f32tof16(asfloat(in0.Load(tid.x * 4))));
    out1.Store(tid.x * 4, asuint(f16tof32(in1.Load(tid.x * 4) & 0xFFFF)));
}
