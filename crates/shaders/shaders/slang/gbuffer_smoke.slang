// `gbuffer_smoke` (ADR-0023 verification gate 1), the Slang half: `encode` runs
// `encode_gbuffer` (lib/gbuffer.slang) on every surface and stores the integers the targets
// would hold; `decode` loads the three G-buffer images the GLSL raster pass wrote and stores
// `decode_gbuffer`'s result. The smoke compares both with the Rust twin
// (crates/render/src/smoke/gbuffer.rs).

import gbuffer;

// `SmokeSurface` in crates/render/src/smoke/gbuffer.rs (64 bytes).
struct SmokeSurface
{
    float4 base_metallic;    // rgb linear base colour, a metallic
    float4 normal_roughness; // xyz normal, w perceptual roughness
    float4 model_data_ao;    // xyz model data (physical units), w material AO
    uint4 model_flags_rim;   // x model, y flags, z rim slot, w unused
};

// `SmokeEncoded`: each channel's stored integer (gb1 as its 10/10/10/2 fields).
struct SmokeEncoded
{
    uint4 gb0;
    uint4 gb1;
    uint4 gb2;
};

[[vk::binding(0, 0)]] StructuredBuffer<SmokeSurface> surfaces;
[[vk::binding(1, 0)]] RWStructuredBuffer<SmokeEncoded> encoded;
[[vk::binding(2, 0)]] Texture2D<float4> gb0;
[[vk::binding(3, 0)]] Texture2D<float4> gb1;
[[vk::binding(4, 0)]] Texture2D<float4> gb2;
[[vk::binding(5, 0)]] RWStructuredBuffer<SmokeSurface> decoded;

struct SmokePush
{
    uint width;
    uint height;
};

[[vk::push_constant]]
ConstantBuffer<SmokePush> push;

[shader("compute")]
[numthreads(8, 8, 1)]
void main(uint3 thread : SV_DispatchThreadID)
{
    if (thread.x >= push.width || thread.y >= push.height)
    {
        return;
    }
    uint index = thread.y * push.width + thread.x;
    SmokeSurface surface = surfaces[index];
    SurfaceParams s;
    s.base_color = surface.base_metallic.rgb;
    s.metallic = surface.base_metallic.a;
    s.normal = surface.normal_roughness.xyz;
    s.roughness = surface.normal_roughness.w;
    s.model = surface.model_flags_rim.x;
    s.model_data = surface.model_data_ao.xyz;
    s.ao = surface.model_data_ao.w;
    s.flags = surface.model_flags_rim.y;
    s.rim_slot = surface.model_flags_rim.z;
    GBufferTexels t = encode_gbuffer(s);
    SmokeEncoded e;
    e.gb0 = uint4(unorm_bits(t.gb0.r, 255.0), unorm_bits(t.gb0.g, 255.0),
                  unorm_bits(t.gb0.b, 255.0), unorm_bits(t.gb0.a, 255.0));
    e.gb1 = uint4(unorm_bits(t.gb1.r, 1023.0), unorm_bits(t.gb1.g, 1023.0),
                  unorm_bits(t.gb1.b, 1023.0), unorm_bits(t.gb1.a, 3.0));
    e.gb2 = uint4(unorm_bits(t.gb2.r, 255.0), unorm_bits(t.gb2.g, 255.0),
                  unorm_bits(t.gb2.b, 255.0), unorm_bits(t.gb2.a, 255.0));
    encoded[index] = e;

    int3 texel = int3(int2(thread.xy), 0);
    GBufferTexels stored;
    stored.gb0 = gb0.Load(texel);
    stored.gb1 = gb1.Load(texel);
    stored.gb2 = gb2.Load(texel);
    SurfaceParams d = decode_gbuffer(stored);
    SmokeSurface result;
    result.base_metallic = float4(d.base_color, d.metallic);
    result.normal_roughness = float4(d.normal, d.roughness);
    result.model_data_ao = float4(d.model_data, d.ao);
    result.model_flags_rim = uint4(d.model, d.flags, d.rim_slot, 0);
    decoded[index] = result;
}
