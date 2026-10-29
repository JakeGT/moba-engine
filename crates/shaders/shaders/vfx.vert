#version 450
#extension GL_GOOGLE_include_directive : require

// GPU particles: camera-facing sprites and flipbooks, segments (trails, beams) and
// ground-aligned quads (rings), six vertices per live particle from the compacted list.

#include "vfx.glsl"

layout(location = 0) out vec2 out_uv;
layout(location = 1) out vec4 out_color;
layout(location = 2) out float out_view_depth;
layout(location = 3) flat out uint out_flags;

const vec2 CORNERS[6] = vec2[](
    vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0)
);

void main() {
    Particle p = particles[alive[gl_InstanceIndex]];
    vec2 corner = CORNERS[gl_VertexIndex % 6];
    float t = clamp(p.pos_age.w / max(p.vel_life.w, 1e-4), 0.0, 1.0);
    float size = mix(p.size_spin.x, p.size_spin.y, t);
    float angle = p.seg_rot.w + p.size_spin.z * p.pos_age.w;
    float c = cos(angle);
    float s = sin(angle);
    vec2 rotated = vec2(corner.x * c - corner.y * s, corner.x * s + corner.y * c);
    uint flags = p.misc.x;
    uint shape = flags & VFX_SHAPE_MASK;
    vec3 world;
    vec2 uv = corner * 0.5 + 0.5;
    if (shape == VFX_SHAPE_SEGMENT) {
        vec3 a = p.pos_age.xyz;
        vec3 b = a + p.seg_rot.xyz;
        vec3 to_eye = frame.eye.xyz - (a + b) * 0.5;
        vec3 side = cross(b - a, to_eye);
        float side_length = length(side);
        side = side_length > 1e-5 ? side / side_length : frame.camera_right.xyz;
        world = (corner.x < 0.0 ? a : b) + side * corner.y * size * 0.5;
    } else if (shape == VFX_SHAPE_GROUND) {
        world = p.pos_age.xyz + vec3(rotated.x, 0.0, rotated.y) * size * 0.5;
        world.y += 0.08;
    } else {
        world = p.pos_age.xyz
            + (frame.camera_right.xyz * rotated.x + frame.camera_up.xyz * rotated.y) * size * 0.5;
    }
    vec4 clip = frame.view_proj * vec4(world, 1.0);
    gl_Position = clip;
    out_uv = vfx_atlas_uv(p, uv, t);
    out_color = vfx_legible(vfx_color(p, t), flags, t);
    out_view_depth = clip.w;
    // Ground quads lie on the terrain: soft fading would erase them. Enemy telegraphs are
    // drawn hard too, so the soft fade can never take them below their opacity floor.
    bool telegraph = ((flags >> VFX_ROLE_SHIFT) & 3u) == VFX_ROLE_TELEGRAPH;
    out_flags = (shape == VFX_SHAPE_GROUND || telegraph) ? (flags | VFX_FLAG_HARD) : flags;
}
