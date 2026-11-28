// Our own HLSL (xdxc oracle, v_div_fixup): the f32 quotient of every pair of a table of 32 special values, a[i] / b[j],
// thread = i * 32 + j. Zeros, infinities, NaNs of both signs and payloads, denormals, the largest and smallest normals.
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=1))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(tbl, 0, 0)
DUAL_UAV(dst, 1, 0)

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    DUAL_BIND_SRV(tbl, 0, 0)
    DUAL_BIND_UAV(dst, 1, 0)
    float a = asfloat(tbl.Load((tid.x >> 5) * 4));
    float b = asfloat(tbl.Load((tid.x & 31) * 4));
    dst.Store(tid.x * 4, asuint(a / b));
}
