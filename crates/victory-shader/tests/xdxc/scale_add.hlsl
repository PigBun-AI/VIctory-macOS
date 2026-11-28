// Our own HLSL (xdxc oracle, harness check): dst[i] = src[i] * 3 + c.x; dst2[i] = src[i] ^ c.y.
// The simplest whole program: a root-constant block, one raw SRV, two raw UAVs in one descriptor table.
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=2))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(dst, 1, 0)
DUAL_UAV(dst2, 2, 1)

DUAL_ROOT(RS)
[numthreads(64, 1, 1)]
void main(uint3 tid : SV_DispatchThreadID)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(dst, 1, 0)
    DUAL_BIND_UAV(dst2, 2, 1)
    uint v = src.Load(tid.x * 4);
    dst.Store(tid.x * 4, v * 3 + params.x);
    dst2.Store(tid.x * 4, v ^ params.y);
}
