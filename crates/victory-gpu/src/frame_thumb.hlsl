// Each group covers four adjacent output cells; sixteen lanes share the
// source footprint of each cell. The sole writer packs the four gray bytes.
Texture2D<float4> image : register(t0);
RWByteAddressBuffer outputBytes : register(u0);
groupshared float partial[64];
[RootSignature("DescriptorTable(SRV(t0)), UAV(u0)")]
[numthreads(64,1,1)]
void main(uint group : SV_GroupID, uint lane : SV_GroupIndex) {
    uint w,h; image.GetDimensions(w,h);
    bool band = group >= 576;
    uint cols = band ? 256 : 64, rows = band ? 24 : 36;
    uint pixel = (band ? group-576 : group)*4 + lane/16;
    uint2 cell = uint2(pixel%cols,pixel/cols);
    float2 size = float2(w,band ? h/5.0 : h)/float2(cols,rows);
    float2 lo = cell*size + float2(0,band ? h*0.8 : 0);
    float2 hi = min(lo+size,float2(w,h));
    float sum=0;
    for (uint y=(uint)lo.y+(lane%16)/4;y<(uint)ceil(hi.y);y+=4)
        for(uint x=(uint)lo.x+lane%4;x<(uint)ceil(hi.x);x+=4) {
            float2 p=float2(x,y);
            float2 weight=min(hi,p+1)-max(lo,p);
            sum += weight.x*weight.y*dot(saturate(image.Load(int3(x,y,0)).rgb),float3(0.2126,0.7152,0.0722));
        }
    partial[lane] = sum;
    [unroll] for(uint step=8;step>0;step>>=1) {
        GroupMemoryBarrierWithGroupSync();
        if((lane%16)<step) partial[lane] += partial[lane+step];
    }
    GroupMemoryBarrierWithGroupSync();
    if(lane==0) {
        uint packed=0;
        [unroll] for(uint i=0;i<4;i++)
            packed |= ((uint)round(saturate(partial[i*16]/(size.x*size.y))*255)) << (8*i);
        outputBytes.Store(group*4,packed);
    }
}
