// The Tiered material table (ADR-0023 §6d): one `MaterialRecord` (64 bytes) per material in a
// host-written buffer the G-buffer stage reads by its device address, indexed by the draw's
// material. The twin of `MaterialRecord` in crates/render/src/material.rs; the
// `slang_material_record_matches_rust` test compares the offsets `slangc -reflection-json`
// reports for this struct with the Rust ones.
//
// Textures are bindless handles (`BindlessTable`, ADR-0020 §3a), two u16 per word. The model
// word packs the model data as three UNORM8 G-buffer channels (§6c: what `gb2.rgb` stores, the
// material AO at 1 where the model has an AO channel), the model ID and four flag bits: the
// emissive mask, and whether the emissive and the model textures are bound (unbound, they are
// the 1×1 white defaults, so the stage skips their samples).
module material;

public static const uint MATERIAL_FLAG_EMISSIVE_MASK = 1; // metallic_roughness.b masks emissive
public static const uint MATERIAL_FLAG_EMISSIVE_TEXTURE = 2; // an emissive texture is bound
public static const uint MATERIAL_FLAG_MODEL_TEXTURE = 4; // a model texture is bound

public struct MaterialRecord
{
    public float4 base_color_factor; // linear RGBA
    public float3 emissive;          // linear RGB factor
    public float normal_scale;
    public float metallic;
    public float roughness;
    public float team_tint;
    public float dust;               // 0..1, times saturate(n.y)
    public uint base_normal;         // base colour | normal << 16
    public uint mr_mask;             // metallic_roughness | team mask << 16
    public uint emissive_model;      // emissive | model texture << 16
    public uint model;               // data r | g << 8 | b << 16 | model << 24 | flags << 28
};

public uint texture_lo(uint word) { return word & 0xFFFFu; }
public uint texture_hi(uint word) { return word >> 16; }
public uint record_model(MaterialRecord r) { return (r.model >> 24) & 0xFu; }
public uint record_flags(MaterialRecord r) { return r.model >> 28; }
public float3 record_model_bytes(MaterialRecord r)
{
    return float3(float(r.model & 0xFFu), float((r.model >> 8) & 0xFFu),
                  float((r.model >> 16) & 0xFFu)) / 255.0;
}
