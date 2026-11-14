// The renderer's `sharpen` pass (ADR-0021 §3): RCAS after a temporal back end that does not
// sharpen itself (DLSS and XeSS, whose own sharpening is deprecated or absent). One compute
// pass at output resolution reading the back end's output and writing `hdr_out`.
//
// RCAS (FidelityFX, MIT; the algorithm, re-written as in `temporal_taau.slang`): a
// five-tap cross in a reversible tonemapped space, the lobe limited so the kernel's centre
// weight stays positive and scaled by `push.sharpness` (0..1; the pass only runs above 0).

[[vk::binding(0, 0)]] Texture2D<float4> source;
[[vk::binding(1, 0)]] Texture2D<float> exposure;
[[vk::binding(2, 0)]] [format("rgba16f")] RWTexture2D<float4> output;

struct SharpenPush
{
    uint2 output_size;
    float sharpness;
    uint pad;
};

[[vk::push_constant]]
ConstantBuffer<SharpenPush> push;

// Reversible tonemap for RCAS (inputs in 0..1), as TAAU's.
float3 rcas_in(float3 c, float e)
{
    float3 x = max(c, float3(0.0, 0.0, 0.0)) * e;
    return x / (1.0 + max(x.r, max(x.g, x.b)));
}

float3 rcas_out(float3 t, float e)
{
    return t / max(1.0 - max(t.r, max(t.g, t.b)), 1e-4) / e;
}

float3 tap(int2 pixel)
{
    int2 clamped = clamp(pixel, int2(0, 0), int2(push.output_size) - 1);
    return source.Load(int3(clamped, 0)).rgb;
}

[shader("compute")]
[numthreads(16, 16, 1)]
void main(uint3 thread : SV_DispatchThreadID)
{
    int2 own = int2(thread.xy);
    if (any(own >= int2(push.output_size)))
    {
        return;
    }
    float4 centre = source.Load(int3(own, 0));
    float ex = max(exposure.Load(int3(0, 0, 0)), 1e-6);
    float3 b = rcas_in(tap(own + int2(0, -1)), ex);
    float3 d = rcas_in(tap(own + int2(-1, 0)), ex);
    float3 c = rcas_in(centre.rgb, ex);
    float3 f = rcas_in(tap(own + int2(1, 0)), ex);
    float3 h = rcas_in(tap(own + int2(0, 1)), ex);
    float3 mn4 = min(min(b, d), min(f, h));
    float3 mx4 = max(max(b, d), max(f, h));
    float3 hit_min = mn4 / max(4.0 * mx4, float3(1e-5, 1e-5, 1e-5));
    float3 hit_max = (1.0 - mx4) / min(4.0 * mn4 - 4.0, float3(-1e-5, -1e-5, -1e-5));
    float3 lobe3 = max(-hit_min, hit_max);
    // The RCAS limit (0.25 - 1/16) keeps the kernel's centre weight positive.
    float lobe = max(-0.1875, min(max(lobe3.x, max(lobe3.y, lobe3.z)), 0.0)) * push.sharpness;
    float3 sharpened = (lobe * (b + d + f + h) + c) / (4.0 * lobe + 1.0);
    output[own] = float4(rcas_out(saturate(sharpened), ex), centre.a);
}
