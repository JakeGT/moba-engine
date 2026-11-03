#version 450
#extension GL_GOOGLE_include_directive : require

// GPU particles: camera-facing sprites and flipbooks, segments (trails, beams) and
// ground-aligned quads (rings), six vertices per live particle from the compacted list.
//
// Tiered builds (ADR-0021): with `VFX_FRAME_NO_TELEGRAPHS` set the scene's draw skips enemy
// telegraphs, which `VFX_OVERLAY` (the overlay's output-resolution draw, unjittered) draws
// alone; `VFX_TEMPORAL` also writes each vertex's unjittered current and previous clip
// position (the previous at `age - dt` and `position - velocity * dt`, which inverts the
// simulation's position step) for the motion of the fragments the motion-only draw keeps.

#include "vfx.glsl"

layout(location = 0) out vec2 out_uv;
layout(location = 1) out vec4 out_color;
layout(location = 2) out float out_view_depth;
layout(location = 3) flat out uint out_flags;
#ifdef VFX_TEMPORAL
layout(location = 14) out vec4 out_motion_current;
layout(location = 15) out vec4 out_motion_previous;
#endif

const vec2 CORNERS[6] = vec2[](
    vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0)
);

// The world position of `corner` of `p` at age `age` with its position at `position`.
vec3 particle_world(Particle p, vec2 corner, float age, vec3 position) {
    float t = clamp(age / max(p.vel_life.w, 1e-4), 0.0, 1.0);
    float size = mix(p.size_spin.x, p.size_spin.y, t);
    float angle = p.seg_rot.w + p.size_spin.z * age;
    float c = cos(angle);
    float s = sin(angle);
    vec2 rotated = vec2(corner.x * c - corner.y * s, corner.x * s + corner.y * c);
    uint shape = p.misc.x & VFX_SHAPE_MASK;
    vec3 world;
    if (shape == VFX_SHAPE_SEGMENT) {
        vec3 a = position;
        vec3 b = a + p.seg_rot.xyz;
        vec3 to_eye = frame.eye.xyz - (a + b) * 0.5;
        vec3 side = cross(b - a, to_eye);
        float side_length = length(side);
        side = side_length > 1e-5 ? side / side_length : frame.camera_right.xyz;
        world = (corner.x < 0.0 ? a : b) + side * corner.y * size * 0.5;
    } else if (shape == VFX_SHAPE_GROUND) {
        world = position + vec3(rotated.x, 0.0, rotated.y) * size * 0.5;
        world.y += 0.08;
    } else {
        world = position
            + (frame.camera_right.xyz * rotated.x + frame.camera_up.xyz * rotated.y) * size * 0.5;
    }
    return world;
}

void main() {
    Particle p = particles[alive[gl_InstanceIndex]];
    vec2 corner = CORNERS[gl_VertexIndex % 6];
    float t = clamp(p.pos_age.w / max(p.vel_life.w, 1e-4), 0.0, 1.0);
    uint flags = p.misc.x;
    uint shape = flags & VFX_SHAPE_MASK;
    bool telegraph = ((flags >> VFX_ROLE_SHIFT) & 3u) == VFX_ROLE_TELEGRAPH;
#ifdef VFX_OVERLAY
    bool skipped = !telegraph;
#else
    bool skipped = telegraph && (frame.counts.w & VFX_FRAME_NO_TELEGRAPHS) != 0u;
#endif
    vec2 uv = corner * 0.5 + 0.5;
    vec3 world = particle_world(p, corner, p.pos_age.w, p.pos_age.xyz);
#ifdef VFX_OVERLAY
    vec4 clip = frame.view_proj_unjittered * vec4(world, 1.0);
#else
    vec4 clip = frame.view_proj * vec4(world, 1.0);
#endif
    if (skipped) {
        // Degenerate: every vertex of the quad at one point outside the view.
        clip = vec4(0.0, 0.0, -1.0, 1.0);
    }
    gl_Position = clip;
    out_uv = vfx_atlas_uv(p, uv, t);
    out_color = vfx_legible(vfx_color(p, t), flags, t);
    out_view_depth = clip.w;
    // Ground quads lie on the terrain: soft fading would erase them. Enemy telegraphs are
    // drawn hard too, so the soft fade can never take them below their opacity floor.
    out_flags = (shape == VFX_SHAPE_GROUND || telegraph) ? (flags | VFX_FLAG_HARD) : flags;
#ifdef VFX_TEMPORAL
    float dt = frame.camera_right.w;
    vec3 previous = particle_world(p, corner, max(p.pos_age.w - dt, 0.0),
                                   p.pos_age.xyz - p.vel_life.xyz * dt);
    out_motion_current = frame.view_proj_unjittered * vec4(world, 1.0);
    out_motion_previous = frame.prev_view_proj * vec4(previous, 1.0);
#endif
}
