// The G-buffer debug views (ADR-0023 §1f): `MOBA_DEBUG_VIEW=albedo|normal|roughness|metallic|
// model|rim-slot|flags` replaces the upscaler's output with one decoded G-buffer channel,
// after the `temporal` pass. Each output pixel shows the render pixel under its centre
// (nearest, no filtering), divided by the exposure the composite applies, as TAAU's views
// are, so the post chain shows the value before its tonemap.

import gbuffer;

[[vk::binding(0, 0)]] Texture2D<float4> gb0;
[[vk::binding(1, 0)]] Texture2D<float4> gb1;
[[vk::binding(2, 0)]] Texture2D<float4> gb2;
[[vk::binding(3, 0)]] Texture2D<float> exposure;
[[vk::binding(4, 0)]] [format("rgba16f")] RWTexture2D<float4> hdr_out;

// `DebugPush` in crates/render/src/passes/gbuffer_debug.rs (24 bytes).
struct DebugPush
{
    uint2 render_size; // this frame's G-buffer region
    uint2 output_size; // hdr_out's region
    uint view;         // `GBufferView` (1 albedo … 7 flags)
    uint reserved;
};

[[vk::push_constant]]
ConstantBuffer<DebugPush> push;

static const uint VIEW_ALBEDO = 1;
static const uint VIEW_NORMAL = 2;
static const uint VIEW_ROUGHNESS = 3;
static const uint VIEW_METALLIC = 4;
static const uint VIEW_MODEL = 5;
static const uint VIEW_RIM_SLOT = 6;
static const uint VIEW_FLAGS = 7;

// Sixteen distinct colours for model IDs and rim slots (0 black: unlit or no rim).
float3 palette(uint index)
{
    static const float3 colors[16] = {
        float3(0.0, 0.0, 0.0),  float3(0.9, 0.9, 0.9),  float3(0.2, 0.5, 1.0),
        float3(1.0, 0.4, 0.8),  float3(1.0, 0.6, 0.1),  float3(0.6, 0.2, 1.0),
        float3(0.2, 0.85, 0.3), float3(0.3, 0.95, 0.95), float3(1.0, 0.15, 0.15),
        float3(0.6, 0.6, 0.1),  float3(0.1, 0.4, 0.4),  float3(0.5, 0.3, 0.1),
        float3(0.4, 0.4, 0.7),  float3(0.7, 0.4, 0.4),  float3(0.4, 0.7, 0.4),
        float3(0.3, 0.3, 0.3),
    };
    return colors[index & 15];
}

[shader("compute")]
[numthreads(8, 8, 1)]
void main(uint3 thread : SV_DispatchThreadID)
{
    if (thread.x >= push.output_size.x || thread.y >= push.output_size.y)
    {
        return;
    }
    uint2 render = max(push.render_size, uint2(1, 1));
    float2 scale = float2(render) / float2(max(push.output_size, uint2(1, 1)));
    uint2 pixel = min(uint2((float2(thread.xy) + 0.5) * scale), render - 1);
    GBufferTexels t;
    t.gb0 = gb0.Load(int3(pixel, 0));
    t.gb1 = gb1.Load(int3(pixel, 0));
    t.gb2 = gb2.Load(int3(pixel, 0));
    SurfaceParams s = decode_gbuffer(t);
    float3 color = float3(0.0, 0.0, 0.0);
    switch (push.view)
    {
    case VIEW_ALBEDO:
        color = s.base_color;
        break;
    case VIEW_NORMAL:
        // World normal as colour; nothing drawn (all-zero texels) shows black.
        color = (t.gb2.a > 0.0 || t.gb0.a > 0.0 || any(t.gb0.rgb > 0.0)) ? s.normal * 0.5 + 0.5
                                                                          : float3(0.0, 0.0, 0.0);
        break;
    case VIEW_ROUGHNESS:
        color = float3(s.roughness, s.roughness, s.roughness);
        break;
    case VIEW_METALLIC:
        color = float3(s.metallic, s.metallic, s.metallic);
        break;
    case VIEW_MODEL:
        color = palette(s.model);
        break;
    case VIEW_RIM_SLOT:
        color = palette(s.rim_slot == 0 ? 0 : s.rim_slot + 8);
        break;
    case VIEW_FLAGS:
        // Red UNIT, green NO_FOG_OF_WAR, blue reserved.
        color = float3(
            (s.flags & GBUFFER_FLAG_UNIT) != 0 ? 1.0 : 0.0,
            (s.flags & GBUFFER_FLAG_NO_FOG_OF_WAR) != 0 ? 1.0 : 0.0,
            (s.flags & GBUFFER_FLAG_RESERVED) != 0 ? 1.0 : 0.0);
        break;
    default:
        break;
    }
    float exposure_value = max(exposure.Load(int3(0, 0, 0)), 1e-6);
    hdr_out[thread.xy] = float4(color / exposure_value, 1.0);
}
