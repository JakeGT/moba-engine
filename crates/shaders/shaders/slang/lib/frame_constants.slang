// The per-frame camera and lighting constants (set 0, binding 0) for Slang passes.
//
// A field-for-field mirror of `CameraBlock` in `../include/camera.glsl`, so Slang and GLSL
// passes can share one uniform buffer. Layout must match `CameraUniform` in
// crates/render/src/scene.rs: the `slang_frame_constants_match_camera_uniform` test there
// compares the offsets and sizes `slangc -reflection-json` reports for this struct with the
// Rust ones (ADR-0020 §4c), so drift fails a test, not a frame. Field comments live in
// camera.glsl; keep the two declarations in the same order.
module frame_constants;

public static const uint MOBA_VISION_CIRCLES = 16;
public static const uint MOBA_VISION_WALLS = 32;
public static const uint MOBA_OUTLINE_SLOTS = 8;

public struct FrameConstants
{
    public float4x4 view_proj;
    public float4 camera_pos;
    public float4 light_direction;
    public float4 ambient;
    public float4 terrain;
    public float4 vision_circles[MOBA_VISION_CIRCLES];
    public uint4 vision_count;
    public float4 vision_walls[MOBA_VISION_WALLS];
    public uint4 wall_count;
    // R2a lighting (ADR-0011).
    public float4x4 light_view_proj;
    public float4 sun_color;
    public float4 shadow_params;
    public float4 shadow_extra;
    public float4 ibl_params;
    public float4 style;
    public float4 rim_color;
    public float4 specular_tint;
    public float4 ambient_ground;
    public float4 sh[9];
    // R3a fog of war and atmosphere (ADR-0014).
    public float4 fog_rect;
    public float4 fog_color;
    public float4 fog_params;
    public float4 haze_color;
    public float4 haze_params;
    public float4 sky_zenith;
    public float4 sky_horizon;
    // R6 post-processing (ADR-0015).
    public float4 post_params;
    public float4 post_clamp;
    // R2b (ADR-0016).
    public float4x4 inv_view_proj;
    public float4 viewport;
    public float4 light_grid;
    public uint4 light_info;
    public float4 occlusion;
    public float4 baked_ao;
    public float4 contact;
    public float4 outline_palette[MOBA_OUTLINE_SLOTS];
    // N1 temporal (ADR-0021 §1, §2).
    public float4x4 view_proj_unjittered;
    public float4x4 prev_view_proj;
    public float4 jitter;
    public float4 temporal;
}
