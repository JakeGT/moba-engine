// The thin G-buffer's encoding (ADR-0023 §1a, §6c) for the G-buffer fragment stages. The
// twin of `encode` in crates/render/src/gbuffer.rs and `encode_gbuffer` in
// shaders/slang/lib/gbuffer.slang; `gbuffer-smoke` checks the stored bytes against the Rust
// twin (verification gate 1). Keep the three in step.
//
// gb0 R8G8B8A8_UNORM:          rgb base colour (sRGB-encoded here), a metallic
// gb1 A2B10G10R10_UNORM_PACK32: rg octahedral world normal, b perceptual roughness,
//                               a flags (bit 0 UNIT, bit 1 reserved)
// gb2 R8G8B8A8_UNORM:          rgb model data, a model << 4 | NO_FOG_OF_WAR << 3 | rim slot
//
// Every value is quantised here, floor(x * (2^b - 1) + 0.5) / (2^b - 1), so the stored bytes
// do not depend on the device's float-to-UNORM rounding.
#ifndef MOBA_GBUFFER_GLSL
#define MOBA_GBUFFER_GLSL

#define GBUFFER_MODEL_UNLIT 0u
#define GBUFFER_MODEL_STANDARD 1u
#define GBUFFER_MODEL_CLEARCOAT 2u
#define GBUFFER_MODEL_SHEEN 3u
#define GBUFFER_MODEL_ANISOTROPIC 4u
#define GBUFFER_MODEL_THIN_FILM 5u
#define GBUFFER_MODEL_FOLIAGE 6u
#define GBUFFER_MODEL_TRANSMISSIVE_FALLBACK 7u

#define GBUFFER_FLAG_UNIT 1u
#define GBUFFER_FLAG_RESERVED 2u
#define GBUFFER_FLAG_NO_FOG_OF_WAR 4u

const float GBUFFER_MIN_ROUGHNESS = 0.045;
const float GBUFFER_MIN_UNIT_ROUGHNESS = 0.08;
const float GBUFFER_MAX_DIELECTRIC_F0 = 0.16;
const float GBUFFER_PI = 3.14159265358979;

// One surface, decoded (ADR-0023 §6d `SurfaceParams`): model data in physical units.
struct SurfaceParams {
    vec3 base_color;  // linear
    float metallic;
    vec3 normal;      // unit, world space
    float roughness;  // perceptual
    uint model;
    vec3 model_data;
    float ao;
    uint flags;
    uint rim_slot;
};

float gbuffer_quantise(float x, float steps) {
    float clamped = isnan(x) ? 0.0 : clamp(x, 0.0, 1.0);
    return floor(clamped * steps + 0.5) / steps;
}

float gbuffer_srgb(float linear_value) {
    float x = isnan(linear_value) ? 0.0 : clamp(linear_value, 0.0, 1.0);
    return x <= 0.0031308 ? 12.92 * x : 1.055 * pow(x, 1.0 / 2.4) - 0.055;
}

float gbuffer_sign(float x) {
    return x >= 0.0 ? 1.0 : -1.0;
}

// Octahedral map folded about +y (Cigolle et al. 2014), to [0, 1]^2.
vec2 gbuffer_octahedral(vec3 n) {
    float sum = abs(n.x) + abs(n.y) + abs(n.z);
    if (!(sum > 0.0) || isinf(sum)) {
        return vec2(0.5);
    }
    vec2 p = vec2(n.x, n.z) / sum;
    if (n.y < 0.0) {
        p = vec2((1.0 - abs(p.y)) * gbuffer_sign(p.x), (1.0 - abs(p.x)) * gbuffer_sign(p.y));
    }
    return p * 0.5 + 0.5;
}

float gbuffer_roughness(float roughness, uint flags) {
    float floor_value = (flags & GBUFFER_FLAG_UNIT) != 0u
        ? GBUFFER_MIN_UNIT_ROUGHNESS : GBUFFER_MIN_ROUGHNESS;
    return clamp(isnan(roughness) ? 1.0 : roughness, floor_value, 1.0);
}

// The model's data mapped to [0, 1] per channel, with the material AO in its channel.
vec3 gbuffer_model_channels(uint model, vec3 data, float ao) {
    vec3 c = vec3(0.0);
    if (model == GBUFFER_MODEL_STANDARD || model == GBUFFER_MODEL_TRANSMISSIVE_FALLBACK) {
        c = vec3(sqrt(max(data.x, 0.0) / GBUFFER_MAX_DIELECTRIC_F0), ao, 0.0);
    } else if (model == GBUFFER_MODEL_CLEARCOAT) {
        c = vec3(data.x, data.y, ao);
    } else if (model == GBUFFER_MODEL_SHEEN) {
        c = data;
    } else if (model == GBUFFER_MODEL_ANISOTROPIC) {
        float angle = (data.y >= 0.0 && data.y <= GBUFFER_PI)
            ? data.y : data.y - GBUFFER_PI * floor(data.y / GBUFFER_PI);
        c = vec3(data.x, angle / GBUFFER_PI, ao);
    } else if (model == GBUFFER_MODEL_THIN_FILM) {
        c = vec3((data.x - 100.0) / 300.0, data.y - 1.0, ao);
    } else if (model == GBUFFER_MODEL_FOLIAGE) {
        c = vec3(data.x, ao, 0.0);
    }
    return c;
}

// `encode_gbuffer`: the three targets' values, already quantised.
void encode_gbuffer(SurfaceParams s, out vec4 gb0, out vec4 gb1, out vec4 gb2) {
    gb0 = vec4(
        gbuffer_quantise(gbuffer_srgb(s.base_color.r), 255.0),
        gbuffer_quantise(gbuffer_srgb(s.base_color.g), 255.0),
        gbuffer_quantise(gbuffer_srgb(s.base_color.b), 255.0),
        gbuffer_quantise(s.metallic, 255.0));
    vec2 oct = gbuffer_octahedral(s.normal);
    gb1 = vec4(
        gbuffer_quantise(oct.x, 1023.0),
        gbuffer_quantise(oct.y, 1023.0),
        gbuffer_quantise(gbuffer_roughness(s.roughness, s.flags), 1023.0),
        float(s.flags & (GBUFFER_FLAG_UNIT | GBUFFER_FLAG_RESERVED)) / 3.0);
    vec3 c = gbuffer_model_channels(s.model, s.model_data, s.ao);
    uint alpha = (s.model << 4) | ((s.flags & GBUFFER_FLAG_NO_FOG_OF_WAR) != 0u ? 8u : 0u)
        | (s.rim_slot & 7u);
    gb2 = vec4(
        gbuffer_quantise(c.r, 255.0),
        gbuffer_quantise(c.g, 255.0),
        gbuffer_quantise(c.b, 255.0),
        float(alpha) / 255.0);
}

#endif
