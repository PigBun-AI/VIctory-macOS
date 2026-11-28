// Our own HLSL (xdxc oracle): quad operations in a compute group of 8x8 threads (quads are the 2x2 neighbours of the group).
// out[(y * 16 + x) * 4 + k] for the thread at (x, y) of a 2x2 grid of groups: k=0 across X, 1 across Y, 2 diagonal, 3 lane 2 of the quad,
// 4 the value read at quad lane 1 only by threads whose x is odd (divergent source), then the plain value.
#define RS "RootFlags(0), RootConstants(num32BitConstants=4, b0), DescriptorTable(SRV(t0, numDescriptors=1), UAV(u0, numDescriptors=1))"
cbuffer Params : register(b0) { uint4 params; };
DUAL_SRV(src, 0, 0)
DUAL_UAV(dst, 1, 0)

DUAL_ROOT(RS)
[numthreads(8, 8, 1)]
void main(uint3 tid : SV_DispatchThreadID, uint3 gt : SV_GroupThreadID)
{
    DUAL_BIND_SRV(src, 0, 0)
    DUAL_BIND_UAV(dst, 1, 0)
    uint flat = tid.y * 16 + tid.x;
    uint v = src.Load(flat * 4);
    uint ax = QuadReadAcrossX(v);
    uint ay = QuadReadAcrossY(v);
    uint ad = QuadReadAcrossDiagonal(v);
    uint l2 = QuadReadLaneAt(v, 2);
    uint odd = 0;
    if ((gt.x & 1) != 0)
        odd = QuadReadLaneAt(v, 1);
    uint o = flat * 6 * 4;
    dst.Store(o, ax);
    dst.Store(o + 4, ay);
    dst.Store(o + 8, ad);
    dst.Store(o + 12, l2);
    dst.Store(o + 16, odd);
    dst.Store(o + 20, v);
}
