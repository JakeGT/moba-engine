// GPU particles (ADR-0013): the particle record, the per-frame block and the buffers of the
// VFX passes. Layouts must match `crates/render/src/passes/vfx.rs` (`GpuParticle`,
// `VfxUniform`) and the args/alive layout documented there.
//
// Compute shaders include this with VFX_COMPUTE defined (writable buffers); vertex shaders
// get read-only buffers, so no vertexPipelineStoresAndAtomics is needed.
#ifndef MOBA_VFX_GLSL
#define MOBA_VFX_GLSL

struct Particle {
    vec4 pos_age;    // world position, age (s)
    vec4 vel_life;   // velocity (m/s), lifetime (s); <= 0 is a free slot
    vec4 seg_rot;    // segment end offset (m), rotation (rad)
    vec4 size_spin;  // start size, end size (m), spin (rad/s), drag (1/s)
    uvec4 colors;    // packHalf2x16: start rg, start ba, end rg, end ba
    uvec4 misc;      // x flags, y sprite, z gravity (float bits), w slot
};

// misc.x
#define VFX_SHAPE_MASK 3u
#define VFX_SHAPE_BILLBOARD 0u
#define VFX_SHAPE_SEGMENT 1u
#define VFX_SHAPE_GROUND 2u
#define VFX_SHAPE_MESH 3u
#define VFX_FLAG_ALPHA 4u
#define VFX_ROLE_SHIFT 3u
#define VFX_ROLE_ALLY 1u
#define VFX_ROLE_TELEGRAPH 2u
#define VFX_MESH_SHIFT 5u
#define VFX_FLAG_HARD 128u

// args[] layout (uints)
#define VFX_ARGS_SPRITE 0u        // VkDrawIndirectCommand
#define VFX_ARGS_MESH 4u          // 4 x VkDrawIndexedIndirectCommand (5 uints each)
#define VFX_COUNT_SPRITE 24u
#define VFX_COUNT_MESH 25u        // 4 counters
#define VFX_STAT_PIXELS 29u
#define VFX_STAT_ALIVE 30u
#define VFX_STAT_SPAWNED 31u
#define VFX_MESH_SLOTS 4u

layout(std140, set = 0, binding = 0) uniform VfxFrame {
    mat4 view_proj;
    vec4 camera_right;   // xyz, w = dt (s)
    vec4 camera_up;      // xyz, w = time (s)
    vec4 eye;            // xyz, w = soft-particle distance (m, 0 = off)
    vec4 viewport;       // width, height, 1/width, 1/height
    vec4 depth;          // P22, P23 (linear depth = P23 / (d + P22)), ally opacity, telegraph floor
    vec4 light;          // xyz towards the light, w = ambient
    uvec4 counts;        // capacity, spawn count, mesh region capacity, unused
    uvec4 mesh_indices;  // index count per mesh slot (0 = unbound)
} frame;

#ifdef VFX_COMPUTE
layout(std430, set = 0, binding = 1) buffer Particles { Particle particles[]; };
layout(std430, set = 0, binding = 2) readonly buffer Spawns { Particle spawns[]; };
layout(std430, set = 0, binding = 3) buffer Alive { uint alive[]; };
layout(std430, set = 0, binding = 4) buffer Args { uint args[]; };
#else
layout(std430, set = 0, binding = 1) readonly buffer Particles { Particle particles[]; };
layout(std430, set = 0, binding = 3) readonly buffer Alive { uint alive[]; };

vec4 vfx_color(Particle p, float t) {
    vec4 start = vec4(unpackHalf2x16(p.colors.x), unpackHalf2x16(p.colors.y));
    vec4 end = vec4(unpackHalf2x16(p.colors.z), unpackHalf2x16(p.colors.w));
    return mix(start, end, t);
}

// Legibility rules (roadmap section 2): allied effects are dimmed by the viewer's setting,
// enemy telegraphs keep a minimum opacity until the last 15 % of their life.
vec4 vfx_legible(vec4 color, uint flags, float t) {
    uint role = (flags >> VFX_ROLE_SHIFT) & 3u;
    if (role == VFX_ROLE_ALLY) {
        color *= frame.depth.z;
    } else if (role == VFX_ROLE_TELEGRAPH) {
        float floor_alpha = frame.depth.w * (1.0 - smoothstep(0.85, 1.0, t));
        color.a = max(color.a, floor_alpha);
    }
    return color;
}

// Atlas UV of `uv` (0..1) inside the particle's current sprite cell.
vec2 vfx_atlas_uv(Particle p, vec2 uv, float t) {
    uint cell = p.misc.y & 255u;
    uint frames = max((p.misc.y >> 8u) & 255u, 1u);
    float fps = float(p.misc.y >> 16u) / 8.0;
    uint frame_index = 0u;
    if (frames > 1u) {
        frame_index = fps > 0.0
            ? uint(p.pos_age.w * fps) % frames
            : min(uint(t * float(frames)), frames - 1u);
    }
    uint index = min(cell + frame_index, 63u);
    vec2 origin = vec2(float(index % 8u), float(index / 8u));
    return (origin + clamp(uv, vec2(0.02), vec2(0.98))) / 8.0;
}
#endif

#endif
