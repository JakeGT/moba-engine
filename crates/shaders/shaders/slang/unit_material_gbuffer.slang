// The textured units' G-buffer fragment stage on the deferred Tiered frame (ADR-0023 §6d,
// N2c): the first Slang fragment stage, behind the GLSL `unit_textured_gbuffer.vert` and
// `skinned_textured_gbuffer.vert` (the GLSL-vertex / Slang-fragment interface test in
// crates/shaders checks the locations). The material comes from the material table
// (`material.slang`, by device address and the draw's index in the push constants) and its
// textures from the bindless set (set 2 of this layout). It encodes the material's model
// (§6c) with `encode_gbuffer`, flags UNIT | NO_FOG_OF_WAR. Permutations: `PREPASS_IDS` also
// writes the outline slot (attachment 4), `GBUFFER_HDR` the emissive into `hdr` (4, or 5
// after the IDs).
//
// The surface is the forward `unit.frag`'s textured one (base colour times the factor, tinted
// by the unit colour under the team mask; metallic/roughness times the factors; the normal
// map in the vertex tangent frame; the emissive), so the deferred and the forward frame
// agree (verification gate 2), plus what only Tiered evaluates: the model, the emissive
// texture, the model texture and the dust layer (§6a).
import frame_constants;
import gbuffer;
import material;

[[vk::binding(0, 0)]] ConstantBuffer<FrameConstants> camera;
// The bindless set (ADR-0020 §3a), at set 2 of the material layout.
[[vk::binding(0, 2)]] SamplerState bindless_samplers[];
[[vk::binding(2, 2)]] Texture2D<float4> bindless_textures[];

// Specialisation constants, set by the renderer from its own values (so they cannot drift):
// the material sampler's bindless slot (`bindless::MATERIAL_SAMPLER`: repeat, trilinear, the
// tier's anisotropy, the `Materials` sampler the forward path uses) and the table's length
// (`material::MAX_MATERIALS`), which bounds the draw's index.
[vk::constant_id(0)] const uint MATERIAL_SAMPLER = 12;
[vk::constant_id(1)] const uint MAX_MATERIALS = 64;
// The dust layer's colour (§6a), ash, until a map carries its own.
static const float3 DUST_COLOR = float3(0.30, 0.28, 0.25);

// `MaterialPush` in crates/render/src/passes/scene.rs (32 bytes): the vertex stage's 16-byte
// `SkinPush`, then the table's address and this draw's material.
struct MaterialPush
{
    uint4 skin;
    MaterialRecord *table;
    uint material;
    uint reserved;
};
[[vk::push_constant]] ConstantBuffer<MaterialPush> push;

struct VertexOutput
{
    [[vk::location(0)]] float3 normal : NORMAL;
    [[vk::location(1)]] float3 color : COLOR;
    [[vk::location(2)]] float3 world_pos : WORLD_POSITION;
    [[vk::location(3)]] float2 uv : TEXCOORD;
    [[vk::location(4)]] float4 tangent : TANGENT;
    [[vk::location(13)]] nointerpolation uint outline_slot : OUTLINE_SLOT;
    [[vk::location(14)]] float4 motion_current : MOTION_CURRENT;
    [[vk::location(15)]] float4 motion_previous : MOTION_PREVIOUS;
};

struct GBufferOutput
{
    float4 motion : SV_Target0;
    float4 gb0 : SV_Target1;
    float4 gb1 : SV_Target2;
    float4 gb2 : SV_Target3;
#ifdef PREPASS_IDS
    uint id : SV_Target4;
#ifdef GBUFFER_HDR
    float4 hdr : SV_Target5;
#endif
#elif defined(GBUFFER_HDR)
    float4 hdr : SV_Target4;
#endif
};

float4 sample_material(uint handle, float2 uv)
{
    return bindless_textures[handle].SampleBias(bindless_samplers[MATERIAL_SAMPLER], uv,
                                                camera.temporal.x);
}

// `motion_pixels` in include/motion.glsl.
float2 motion_pixels(float4 current, float4 previous)
{
    float2 ndc_current = current.xy / current.w;
    float2 ndc_previous = previous.xy / previous.w;
    return (ndc_previous - ndc_current) * float2(0.5, -0.5) * camera.temporal.yz;
}

// `material_normal` in include/material.glsl: the normal map in the vertex tangent frame
// (glTF / MikkTSpace: bitangent = cross(normal, tangent.xyz) * tangent.w).
float3 material_normal(float3 normal, float4 tangent, float2 xy_sample, float scale)
{
    float2 xy = (xy_sample * 2.0 - 1.0) * scale;
    float3 tangent_normal = float3(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
    float3 t = tangent.xyz - normal * dot(normal, tangent.xyz);
    float length_squared = dot(t, t);
    if (length_squared <= 1e-12)
    {
        return normal;
    }
    t *= rsqrt(length_squared);
    float3 b = cross(normal, t) * (tangent.w < 0.0 ? -1.0 : 1.0);
    return normalize(t * tangent_normal.x + b * tangent_normal.y + normal * tangent_normal.z);
}

// Duff et al. 2017's orthonormal basis from a unit normal: `model_basis` in
// include/shading_models.glsl, which the deferred lighting rebuilds from the stored normal.
void model_basis(float3 n, out float3 t, out float3 b)
{
    float s = n.z >= 0.0 ? 1.0 : -1.0;
    float a = -1.0 / (s + n.z);
    float c = n.x * n.y * a;
    t = float3(1.0 + s * n.x * n.x * a, s * c, -s * n.x);
    b = float3(c, s + n.y * n.y * a, -n.y);
}

// The anisotropy's direction as the G-buffer stores it (§6c, amendment 3): the authored
// rotation turns the mesh's tangent about the normal (`KHR_materials_anisotropy`), and the
// result is stored as its angle, modulo π, in the basis the lighting rebuilds from the stored
// (quantised) normal. The basis flips across n.z = 0, the angle with it, so the direction is
// as continuous as the mesh's tangents. Without a usable tangent, the rotation is taken in
// that basis.
float anisotropy_angle(float3 normal, float4 tangent, float rotation)
{
    float2 oct = floor(octahedral_encode(normal) * 1023.0 + 0.5) / 1023.0;
    float3 stored = octahedral_decode(oct);
    float3 t0;
    float3 b0;
    model_basis(stored, t0, b0);
    float3 t = tangent.xyz - stored * dot(stored, tangent.xyz);
    float length_squared = dot(t, t);
    if (length_squared <= 1e-12)
    {
        return rotation;
    }
    t *= rsqrt(length_squared);
    float3 b = cross(stored, t) * (tangent.w < 0.0 ? -1.0 : 1.0);
    float3 direction = cos(rotation) * t + sin(rotation) * b;
    float angle = atan2(dot(direction, b0), dot(direction, t0));
    return angle < 0.0 ? angle + 3.14159265358979 : angle;
}

[shader("fragment")]
GBufferOutput main(VertexOutput input)
{
    MaterialRecord record = push.table[min(push.material, MAX_MATERIALS - 1)];
    float3 geometric = normalize(input.normal);
    float4 base = sample_material(texture_lo(record.base_normal), input.uv)
        * record.base_color_factor;
    float tint = sample_material(texture_hi(record.mr_mask), input.uv).r * record.team_tint;
    float3 albedo = base.rgb * lerp(float3(1.0), input.color, tint);
    float4 mr = sample_material(texture_lo(record.mr_mask), input.uv);
    float metallic = clamp(mr.r * record.metallic, 0.0, 1.0);
    float roughness = clamp(mr.g * record.roughness, 0.0, 1.0);
    float3 normal = material_normal(
        geometric, input.tangent,
        sample_material(texture_hi(record.base_normal), input.uv).rg, record.normal_scale);
    uint flags = record_flags(record);
    float mask = (flags & MATERIAL_FLAG_EMISSIVE_MASK) != 0 ? mr.b : 1.0;
    // Unbound emissive and model textures are the 1×1 white defaults: their samples are
    // skipped (the flags are uniform per draw).
    float3 emissive = record.emissive * mask;
    if ((flags & MATERIAL_FLAG_EMISSIVE_TEXTURE) != 0)
    {
        emissive *= sample_material(texture_lo(record.emissive_model), input.uv).rgb;
    }
    // The dust layer (§6a): amount times how much the surface faces up.
    float dust = record.dust * saturate(normal.y);
    albedo = lerp(albedo, DUST_COLOR, dust);

    // The model's data: the record's channels times the model texture's (§6f), in the
    // G-buffer's units, then to the physical units `encode_gbuffer` takes.
    uint model = record_model(record);
    float3 bytes = record_model_bytes(record);
    float3 texel = float3(1.0);
    if ((flags & MATERIAL_FLAG_MODEL_TEXTURE) != 0)
    {
        texel = sample_material(texture_hi(record.emissive_model), input.uv).rgb;
    }
    float3 c = bytes * texel;
    SurfaceParams s;
    s.base_color = albedo;
    s.metallic = metallic;
    s.normal = normal;
    s.roughness = roughness;
    s.model = model;
    s.ao = 1.0;
    s.model_data = float3(0.0);
    if (model == GBUFFER_MODEL_STANDARD || model == GBUFFER_MODEL_TRANSMISSIVE_FALLBACK)
    {
        s.model_data = float3(0.16 * c.x * c.x, 0.0, 0.0);
        s.ao = c.y;
    }
    else if (model == GBUFFER_MODEL_CLEARCOAT)
    {
        s.model_data = float3(c.x, c.y, 0.0);
        s.ao = c.z;
    }
    else if (model == GBUFFER_MODEL_ANISOTROPIC)
    {
        s.model_data = float3(c.x, anisotropy_angle(normal, input.tangent, c.y * 3.14159265358979),
                              0.0);
        s.ao = c.z;
    }
    else if (model == GBUFFER_MODEL_SHEEN)
    {
        s.model_data = c;
    }
    else if (model == GBUFFER_MODEL_THIN_FILM)
    {
        // r and b are the thickness range's ends; the model texture's r picks between them,
        // its g scales the film's IOR and its b is the material AO, as on the other models.
        float thickness = lerp(bytes.x, bytes.z, texel.r);
        s.model_data = float3(100.0 + 300.0 * thickness, 1.0 + c.y, 0.0);
        s.ao = texel.b;
    }
    else if (model == GBUFFER_MODEL_FOLIAGE)
    {
        s.model_data = float3(c.x, 0.0, 0.0);
        s.ao = c.y;
    }
    s.flags = GBUFFER_FLAG_UNIT | GBUFFER_FLAG_NO_FOG_OF_WAR;
    s.rim_slot = 0; // Team rims arrive in N2e (§8).

    GBufferTexels t = encode_gbuffer(s);
    GBufferOutput output;
    output.motion = float4(motion_pixels(input.motion_current, input.motion_previous), 0.0, 1.0);
    output.gb0 = t.gb0;
    output.gb1 = t.gb1;
    output.gb2 = t.gb2;
#ifdef PREPASS_IDS
    output.id = input.outline_slot == 0 ? 8 : input.outline_slot;
#endif
#ifdef GBUFFER_HDR
    output.hdr = float4(min(emissive, float3(32768.0)), 1.0);
#endif
    return output;
}
