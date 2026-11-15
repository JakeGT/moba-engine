// The thin G-buffer's encoding (ADR-0023 §1a, §6c, §6d): `SurfaceParams`, `encode_gbuffer`
// and `decode_gbuffer`. The twin of crates/render/src/gbuffer.rs and of
// shaders/include/gbuffer.glsl (which the GLSL G-buffer fragment stages use until N2c ports
// them); `gbuffer-smoke` checks all three against each other (verification gate 1). Keep them
// in step.
//
// gb0 R8G8B8A8_UNORM:          rgb base colour (sRGB-encoded), a metallic
// gb1 A2B10G10R10_UNORM_PACK32: rg octahedral world normal, b perceptual roughness,
//                               a flags (bit 0 UNIT, bit 1 reserved)
// gb2 R8G8B8A8_UNORM:          rgb model data, a model << 4 | NO_FOG_OF_WAR << 3 | rim slot
module gbuffer;

public static const uint GBUFFER_MODEL_UNLIT = 0;
public static const uint GBUFFER_MODEL_STANDARD = 1;
public static const uint GBUFFER_MODEL_CLEARCOAT = 2;
public static const uint GBUFFER_MODEL_SHEEN = 3;
public static const uint GBUFFER_MODEL_ANISOTROPIC = 4;
public static const uint GBUFFER_MODEL_THIN_FILM = 5;
public static const uint GBUFFER_MODEL_FOLIAGE = 6;
public static const uint GBUFFER_MODEL_TRANSMISSIVE_FALLBACK = 7;

public static const uint GBUFFER_FLAG_UNIT = 1;
public static const uint GBUFFER_FLAG_RESERVED = 2;
public static const uint GBUFFER_FLAG_NO_FOG_OF_WAR = 4;

static const float MIN_ROUGHNESS = 0.045;
static const float MIN_UNIT_ROUGHNESS = 0.08;
static const float MAX_DIELECTRIC_F0 = 0.16;
static const float PI = 3.14159265358979;

// One surface, decoded: model data in physical units (§6c).
public struct SurfaceParams
{
    public float3 base_color; // linear
    public float metallic;
    public float3 normal;     // unit, world space
    public float roughness;   // perceptual
    public uint model;
    public float3 model_data;
    public float ao;
    public uint flags;
    public uint rim_slot;
};

// The three targets' values as written or sampled (UNORM, so each in [0, 1]).
public struct GBufferTexels
{
    public float4 gb0;
    public float4 gb1;
    public float4 gb2;
};

float quantise(float x, float steps)
{
    float clamped = isnan(x) ? 0.0 : clamp(x, 0.0, 1.0);
    return floor(clamped * steps + 0.5) / steps;
}

float srgb_encode(float linear_value)
{
    float x = isnan(linear_value) ? 0.0 : clamp(linear_value, 0.0, 1.0);
    return x <= 0.0031308 ? 12.92 * x : 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

float srgb_decode(float encoded)
{
    return encoded <= 0.04045 ? encoded / 12.92 : pow((encoded + 0.055) / 1.055, 2.4);
}

float sign_not_zero(float x)
{
    return x >= 0.0 ? 1.0 : -1.0;
}

// Octahedral map folded about +y (Cigolle et al. 2014), to [0, 1]^2.
public float2 octahedral_encode(float3 n)
{
    float sum = abs(n.x) + abs(n.y) + abs(n.z);
    if (!(sum > 0.0) || isinf(sum))
    {
        return float2(0.5, 0.5);
    }
    float2 p = float2(n.x, n.z) / sum;
    if (n.y < 0.0)
    {
        p = float2((1.0 - abs(p.y)) * sign_not_zero(p.x), (1.0 - abs(p.x)) * sign_not_zero(p.y));
    }
    return p * 0.5 + 0.5;
}

public float3 octahedral_decode(float2 encoded)
{
    float2 p = encoded * 2.0 - 1.0;
    float3 n = float3(p.x, 1.0 - abs(p.x) - abs(p.y), p.y);
    float t = max(-n.y, 0.0);
    n.x += n.x >= 0.0 ? -t : t;
    n.z += n.z >= 0.0 ? -t : t;
    return n / sqrt(n.x * n.x + n.y * n.y + n.z * n.z);
}

float stored_roughness(float roughness, uint flags)
{
    float floor_value = (flags & GBUFFER_FLAG_UNIT) != 0 ? MIN_UNIT_ROUGHNESS : MIN_ROUGHNESS;
    return clamp(isnan(roughness) ? 1.0 : roughness, floor_value, 1.0);
}

float3 model_channels(uint model, float3 data, float ao)
{
    float3 c = float3(0.0, 0.0, 0.0);
    if (model == GBUFFER_MODEL_STANDARD || model == GBUFFER_MODEL_TRANSMISSIVE_FALLBACK)
    {
        c = float3(sqrt(max(data.x, 0.0) / MAX_DIELECTRIC_F0), ao, 0.0);
    }
    else if (model == GBUFFER_MODEL_CLEARCOAT)
    {
        c = float3(data.x, data.y, ao);
    }
    else if (model == GBUFFER_MODEL_SHEEN)
    {
        c = data;
    }
    else if (model == GBUFFER_MODEL_ANISOTROPIC)
    {
        float angle = (data.y >= 0.0 && data.y <= PI) ? data.y : data.y - PI * floor(data.y / PI);
        c = float3(data.x, angle / PI, ao);
    }
    else if (model == GBUFFER_MODEL_THIN_FILM)
    {
        c = float3((data.x - 100.0) / 300.0, data.y - 1.0, ao);
    }
    else if (model == GBUFFER_MODEL_FOLIAGE)
    {
        c = float3(data.x, ao, 0.0);
    }
    return c;
}

// `encode_gbuffer`: one surface to the targets' values, quantised (so the stored bytes do not
// depend on the device's float-to-UNORM rounding).
public GBufferTexels encode_gbuffer(SurfaceParams s)
{
    GBufferTexels t;
    t.gb0 = float4(
        quantise(srgb_encode(s.base_color.r), 255.0),
        quantise(srgb_encode(s.base_color.g), 255.0),
        quantise(srgb_encode(s.base_color.b), 255.0),
        quantise(s.metallic, 255.0));
    float2 oct = octahedral_encode(s.normal);
    t.gb1 = float4(
        quantise(oct.x, 1023.0),
        quantise(oct.y, 1023.0),
        quantise(stored_roughness(s.roughness, s.flags), 1023.0),
        float(s.flags & (GBUFFER_FLAG_UNIT | GBUFFER_FLAG_RESERVED)) / 3.0);
    float3 c = model_channels(s.model, s.model_data, s.ao);
    uint alpha = (s.model << 4) | ((s.flags & GBUFFER_FLAG_NO_FOG_OF_WAR) != 0 ? 8u : 0u)
        | (s.rim_slot & 7u);
    t.gb2 = float4(quantise(c.r, 255.0), quantise(c.g, 255.0), quantise(c.b, 255.0),
                   float(alpha) / 255.0);
    return t;
}

// The stored integer of a UNORM value with `steps` levels.
public uint unorm_bits(float x, float steps)
{
    return uint(clamp(x, 0.0, 1.0) * steps + 0.5);
}

// `decode_gbuffer`: sampled texels back to a surface. A reserved model ID (8–15) decodes with
// its ID and the model data as stored.
public SurfaceParams decode_gbuffer(GBufferTexels t)
{
    SurfaceParams s;
    s.base_color = float3(
        srgb_decode(float(unorm_bits(t.gb0.r, 255.0)) / 255.0),
        srgb_decode(float(unorm_bits(t.gb0.g, 255.0)) / 255.0),
        srgb_decode(float(unorm_bits(t.gb0.b, 255.0)) / 255.0));
    s.metallic = float(unorm_bits(t.gb0.a, 255.0)) / 255.0;
    s.normal = octahedral_decode(float2(
        float(unorm_bits(t.gb1.r, 1023.0)) / 1023.0, float(unorm_bits(t.gb1.g, 1023.0)) / 1023.0));
    s.roughness = float(unorm_bits(t.gb1.b, 1023.0)) / 1023.0;
    uint alpha = unorm_bits(t.gb2.a, 255.0);
    s.model = alpha >> 4;
    s.rim_slot = alpha & 7;
    s.flags = unorm_bits(t.gb1.a, 3.0) | ((alpha & 8) != 0 ? GBUFFER_FLAG_NO_FOG_OF_WAR : 0);
    float3 c = float3(
        float(unorm_bits(t.gb2.r, 255.0)) / 255.0,
        float(unorm_bits(t.gb2.g, 255.0)) / 255.0,
        float(unorm_bits(t.gb2.b, 255.0)) / 255.0);
    s.ao = 1.0;
    s.model_data = c;
    switch (s.model)
    {
    case GBUFFER_MODEL_UNLIT:
        s.model_data = float3(0.0, 0.0, 0.0);
        break;
    case GBUFFER_MODEL_STANDARD:
    case GBUFFER_MODEL_TRANSMISSIVE_FALLBACK:
        s.model_data = float3(MAX_DIELECTRIC_F0 * c.x * c.x, 0.0, 0.0);
        s.ao = c.y;
        break;
    case GBUFFER_MODEL_CLEARCOAT:
        s.model_data = float3(c.x, c.y, 0.0);
        s.ao = c.z;
        break;
    case GBUFFER_MODEL_SHEEN:
        break;
    case GBUFFER_MODEL_ANISOTROPIC:
        s.model_data = float3(c.x, c.y * PI, 0.0);
        s.ao = c.z;
        break;
    case GBUFFER_MODEL_THIN_FILM:
        s.model_data = float3(100.0 + c.x * 300.0, 1.0 + c.y, 0.0);
        s.ao = c.z;
        break;
    case GBUFFER_MODEL_FOLIAGE:
        s.model_data = float3(c.x, 0.0, 0.0);
        s.ao = c.y;
        break;
    default:
        break;
    }
    return s;
}
