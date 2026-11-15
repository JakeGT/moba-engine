// In-house temporal AA and upscaling (ADR-0021 §3): one compute pass at output resolution
// after the particles and `tc_mask`, before bloom. TAA at ratio 1.0, TAAU up to 3.0.
//
// Karis 2014 (SIGGRAPH Advances) with Playdead's INSIDE TAA (MIT) as a reference only; no code
// is copied. Per output pixel:
//   1. dilated motion: the motion of the nearest-depth render pixel in a 3x3 cross;
//   2. history: a 5-tap Catmull-Rom of the output-resolution history at the reprojected
//      position;
//   3. the upsampled current sample: the render samples around the pixel weighted by a
//      Blackman-Harris kernel (its Gaussian fit) of their jittered distance; the largest
//      weight is the pixel's confidence;
//   4. a variance clip in YCoCg (gamma 1.0, 1.25 on static pixels, narrowed by 0.5 x tc_mask)
//      towards the mean;
//   5. a luminance-weighted blend (1 / (1 + L)) against fireflies.
// Ghosting rules: base history weight 0.9 at native rising to 0.95 at 2x, lowered by the
// current sample's confidence; falling linearly to 0.8 at 8 output pixels of motion per
// frame; rejected on disocclusion (previous linear depth more than 1 % off); at most
// `1 - reactive`. A reset writes the upsampled current colour with weight 1.
//
// RCAS (FidelityFX, MIT; the algorithm, re-written) is fused: each 16x16 group resolves its
// tile plus a one-pixel border into group memory, then sharpens its pixels in a reversible
// tonemapped space. `history_out` keeps the unsharpened value.
//
// Inputs are in the top-left `render_size` of their allocations; `motion` is in render pixels
// from the current sample to its previous position, without jitter (ADR-0021 §2a).

[[vk::binding(0, 0)]] Texture2D<float4> color;
[[vk::binding(1, 0)]] Texture2D<float> depth;
[[vk::binding(2, 0)]] Texture2D<float2> motion;
[[vk::binding(3, 0)]] Texture2D<float> reactive;
[[vk::binding(4, 0)]] Texture2D<float> tc_mask;
[[vk::binding(5, 0)]] Texture2D<float> exposure;
[[vk::binding(6, 0)]] Texture2D<float4> history_in;
[[vk::binding(7, 0)]] [format("rgba16f")] RWTexture2D<float4> history_out;
[[vk::binding(8, 0)]] [format("rgba16f")] RWTexture2D<float4> output;
[[vk::binding(9, 0)]] Texture2D<float> prev_depth_in;
[[vk::binding(10, 0)]] [format("r32f")] RWTexture2D<float> prev_depth_out;
[[vk::binding(11, 0)]] SamplerState linear_clamp;

struct TaauPush
{
    uint2 render_size;
    uint2 output_size;
    float2 jitter;          // render pixels, x right, y down
    uint2 prev_render_size; // the previous frame's render size (its depth's valid region)
    float2 history_scale;   // the history's valid uv region (1, 1: the whole output)
    float p22;              // linear depth = p23 / (depth + p22), reversed-Z depth
    float p23;
    float sharpness;        // RCAS amount 0..1 (0 = off)
    uint flags;             // 1 reset, 2 sharpen, debug view << 8
};

[[vk::push_constant]]
ConstantBuffer<TaauPush> push;

static const uint FLAG_RESET = 1;
static const uint FLAG_SHARPEN = 2;
static const uint DEBUG_MOTION = 1;
static const uint DEBUG_REACTIVE = 2;
static const uint DEBUG_TC = 3;
static const uint DEBUG_DISOCCLUSION = 4;
static const uint DEBUG_CONFIDENCE = 5;

static const int TILE = 16;
static const int BORDER_TILE = TILE + 2;
groupshared float3 resolved[BORDER_TILE * BORDER_TILE];

float luma(float3 c)
{
    return dot(c, float3(0.2126, 0.7152, 0.0722));
}

float3 to_ycocg(float3 c)
{
    return float3(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b);
}

float3 from_ycocg(float3 c)
{
    return float3(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

int2 clamp_render(int2 p)
{
    return clamp(p, int2(0, 0), int2(push.render_size) - 1);
}

float linear_depth(float d)
{
    return push.p23 / (d + push.p22);
}

// Blackman-Harris (its Gaussian fit) of a distance in render pixels.
float kernel(float2 d)
{
    return exp(-2.29 * dot(d, d));
}

// 5-tap Catmull-Rom of the history at `uv` (output-resolution, bilinear taps).
float3 history_catmull_rom(float2 uv)
{
    float2 size = float2(push.output_size);
    float2 position = uv * size;
    float2 centre = floor(position - 0.5) + 0.5;
    float2 f = position - centre;
    float2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    float2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    float2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    float2 w3 = f * f * (-0.5 + 0.5 * f);
    float2 w12 = w1 + w2;
    float2 tc12 = (centre + w2 / w12) / size;
    float2 tc0 = (centre - 1.0) / size;
    float2 tc3 = (centre + 2.0) / size;
    float2 scale = push.history_scale;
    float3 result = history_in.SampleLevel(linear_clamp, float2(tc12.x, tc0.y) * scale, 0).rgb * (w12.x * w0.y)
        + history_in.SampleLevel(linear_clamp, float2(tc0.x, tc12.y) * scale, 0).rgb * (w0.x * w12.y)
        + history_in.SampleLevel(linear_clamp, float2(tc12.x, tc12.y) * scale, 0).rgb * (w12.x * w12.y)
        + history_in.SampleLevel(linear_clamp, float2(tc3.x, tc12.y) * scale, 0).rgb * (w3.x * w12.y)
        + history_in.SampleLevel(linear_clamp, float2(tc12.x, tc3.y) * scale, 0).rgb * (w12.x * w3.y);
    float total = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return max(result / max(total, 1e-5), float3(0.0, 0.0, 0.0));
}

// Resolves one output pixel; writes the history (and, for the group's own pixels, the
// previous depth) when `store`.
float3 resolve_pixel(int2 out_pixel, bool store)
{
    float2 out_size = float2(push.output_size);
    float2 render_size = float2(push.render_size);
    float2 ratio = render_size / out_size;
    // The output pixel centre in render pixels, and the render sample nearest to it.
    float2 q = (float2(out_pixel) + 0.5) * ratio;
    int2 base = clamp_render(int2(floor(q)));
    float exposure_value = max(exposure.Load(int3(0, 0, 0)), 1e-6);

    // 3. The upsampled current sample and 4.'s neighbourhood moments (YCoCg of the
    //    exposure-scaled, tonemapped colour, so bright samples do not dominate).
    float3 sum = float3(0.0, 0.0, 0.0);
    float weight_sum = 0.0;
    float confidence = 0.0;
    float3 m1 = float3(0.0, 0.0, 0.0);
    float3 m2 = float3(0.0, 0.0, 0.0);
    // 1. The nearest depth of the 3x3 cross (reversed-Z, ADR-0023 §1c: larger is nearer).
    float nearest = -1.0;
    int2 nearest_pixel = base;
    for (int y = -1; y <= 1; y++)
    {
        for (int x = -1; x <= 1; x++)
        {
            int2 p = clamp_render(base + int2(x, y));
            float3 c = max(color.Load(int3(p, 0)).rgb, float3(0.0, 0.0, 0.0));
            // The sample at pixel p sees the scene at p + 0.5 - jitter (unjittered).
            float2 d = (float2(p) + 0.5 - push.jitter) - q;
            float w = kernel(d);
            sum += c * w;
            weight_sum += w;
            confidence = max(confidence, w);
            float3 t = to_ycocg(c * exposure_value / (1.0 + luma(c * exposure_value)));
            m1 += t;
            m2 += t * t;
            if (x == 0 || y == 0)
            {
                float z = depth.Load(int3(p, 0));
                if (z > nearest)
                {
                    nearest = z;
                    nearest_pixel = p;
                }
            }
        }
    }
    float3 current = weight_sum > 1e-5 ? sum / weight_sum : color.Load(int3(base, 0)).rgb;
    float2 m = motion.Load(int3(nearest_pixel, 0));
    float react = reactive.Load(int3(base, 0));
    float tc = tc_mask.Load(int3(base, 0));
    float current_depth = linear_depth(depth.Load(int3(base, 0)));
    if (store && all(out_pixel < int2(push.render_size)))
    {
        // The render-resolution previous depth: written by the threads whose output pixel
        // index is a render pixel (render <= output on every axis).
        prev_depth_out[out_pixel] = linear_depth(depth.Load(int3(clamp_render(out_pixel), 0)));
    }

    // 2. Reprojection.
    float2 prev_render = q + m;            // render pixels in the previous frame
    float2 prev_uv = prev_render / render_size;
    bool outside = any(prev_uv < 0.0) || any(prev_uv > 1.0);
    bool reset = (push.flags & FLAG_RESET) != 0;
    float history_weight = 0.0;
    float3 history = current;
    bool disoccluded = false;
    if (!reset && !outside)
    {
        history = history_catmull_rom(prev_uv);
        // Disocclusion: the previous linear depth at the reprojected position.
        int2 prev_pixel = clamp(int2(prev_uv * float2(push.prev_render_size)), int2(0, 0),
                                int2(push.prev_render_size) - 1);
        float previous_depth = prev_depth_in.Load(int3(prev_pixel, 0));
        disoccluded = abs(previous_depth - current_depth) > 0.01 * current_depth;
        // Base weight: 0.9 at native, 0.95 at 2x; the confidence lowers it.
        float upscale = clamp(1.0 / max(ratio.x, 1e-3) - 1.0, 0.0, 1.0);
        history_weight = lerp(0.9, 0.95, upscale) * lerp(1.0, 0.95, confidence);
        // Velocity: linearly to 0.8 at 8 output pixels per frame.
        float speed = length(m / ratio);
        history_weight = min(history_weight, lerp(history_weight, 0.8, saturate(speed / 8.0)));
        history_weight = min(history_weight, 1.0 - react);
        if (disoccluded)
        {
            history_weight = 0.0;
        }
        // 4. Variance clip towards the mean.
        float3 mean = m1 / 9.0;
        float3 sigma = sqrt(max(m2 / 9.0 - mean * mean, float3(0.0, 0.0, 0.0)));
        float gamma = (length(m) < 0.05 ? 1.25 : 1.0) - 0.5 * tc;
        float3 h = to_ycocg(history * exposure_value / (1.0 + luma(history * exposure_value)));
        float3 lo = mean - gamma * sigma;
        float3 hi = mean + gamma * sigma;
        float3 centre = 0.5 * (hi + lo);
        float3 extent = max(0.5 * (hi - lo), float3(1e-5, 1e-5, 1e-5));
        float3 offset = h - centre;
        float3 units = abs(offset / extent);
        float largest = max(units.x, max(units.y, units.z));
        if (largest > 1.0)
        {
            h = centre + offset / largest;
        }
        float3 clipped = from_ycocg(h);
        // Back from the tonemapped space.
        float l = luma(clipped);
        history = max(clipped / max(1.0 - l, 1e-3), float3(0.0, 0.0, 0.0)) / exposure_value;
    }

    // 5. Luminance-weighted blend.
    float wc = (1.0 - history_weight) / (1.0 + luma(current * exposure_value));
    float wh = history_weight / (1.0 + luma(history * exposure_value));
    float3 result = (current * wc + history * wh) / max(wc + wh, 1e-6);
    if (store)
    {
        history_out[out_pixel] = float4(result, 1.0);
    }

    uint view = push.flags >> 8;
    if (view == DEBUG_MOTION)
    {
        return float3(abs(m) * 0.125, 0.0) / exposure_value;
    }
    if (view == DEBUG_REACTIVE)
    {
        return float3(react, react, react) / exposure_value;
    }
    if (view == DEBUG_TC)
    {
        return float3(tc, tc, tc) / exposure_value;
    }
    if (view == DEBUG_DISOCCLUSION)
    {
        return (disoccluded ? float3(1.0, 0.0, 0.0) : float3(0.0, 0.0, 0.0)) / exposure_value;
    }
    if (view == DEBUG_CONFIDENCE)
    {
        return float3(confidence, confidence, confidence) / exposure_value;
    }
    return result;
}

// Reversible tonemap for RCAS (inputs in 0..1).
float3 rcas_in(float3 c, float e)
{
    float3 x = c * e;
    return x / (1.0 + max(x.r, max(x.g, x.b)));
}

float3 rcas_out(float3 t, float e)
{
    return t / max(1.0 - max(t.r, max(t.g, t.b)), 1e-4) / e;
}

[shader("compute")]
[numthreads(16, 16, 1)]
void main(uint3 group : SV_GroupID, uint3 local : SV_GroupThreadID)
{
    int2 origin = int2(group.xy) * TILE;
    int index = int(local.y) * TILE + int(local.x);
    int2 own = origin + int2(local.xy);
    bool inside = all(own < int2(push.output_size));
    // Resolve the tile and its one-pixel border into group memory (324 pixels, 256
    // threads); only the tile's own pixels store history.
    for (int i = index; i < BORDER_TILE * BORDER_TILE; i += TILE * TILE)
    {
        int2 cell = int2(i % BORDER_TILE, i / BORDER_TILE);
        int2 pixel = clamp(origin + cell - 1, int2(0, 0), int2(push.output_size) - 1);
        bool is_own = all(cell >= 1) && all(cell <= TILE) && all(origin + cell - 1 < int2(push.output_size));
        resolved[i] = resolve_pixel(pixel, is_own);
    }
    GroupMemoryBarrierWithGroupSync();
    if (!inside)
    {
        return;
    }
    int2 cell = int2(local.xy) + 1;
    float3 e = resolved[cell.y * BORDER_TILE + cell.x];
    if ((push.flags & FLAG_SHARPEN) == 0 || push.sharpness <= 0.0 || (push.flags >> 8) != 0)
    {
        output[own] = float4(e, 1.0);
        return;
    }
    float ex = max(exposure.Load(int3(0, 0, 0)), 1e-6);
    float3 b = rcas_in(resolved[(cell.y - 1) * BORDER_TILE + cell.x], ex);
    float3 d = rcas_in(resolved[cell.y * BORDER_TILE + cell.x - 1], ex);
    float3 c = rcas_in(e, ex);
    float3 f = rcas_in(resolved[cell.y * BORDER_TILE + cell.x + 1], ex);
    float3 h = rcas_in(resolved[(cell.y + 1) * BORDER_TILE + cell.x], ex);
    float3 mn4 = min(min(b, d), min(f, h));
    float3 mx4 = max(max(b, d), max(f, h));
    float3 hit_min = mn4 / max(4.0 * mx4, float3(1e-5, 1e-5, 1e-5));
    float3 hit_max = (1.0 - mx4) / min(4.0 * mn4 - 4.0, float3(-1e-5, -1e-5, -1e-5));
    float3 lobe3 = max(-hit_min, hit_max);
    // The RCAS limit (0.25 - 1/16) keeps the kernel's centre weight positive.
    float lobe = max(-0.1875, min(max(lobe3.x, max(lobe3.y, lobe3.z)), 0.0)) * push.sharpness;
    float3 sharpened = (lobe * (b + d + f + h) + c) / (4.0 * lobe + 1.0);
    output[own] = float4(rcas_out(saturate(sharpened), ex), 1.0);
}
