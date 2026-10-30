#version 450
#extension GL_GOOGLE_include_directive : require

// Heat haze (ADR-0019): upright camera-facing sprites from the distortion list, six vertices
// per live distortion particle. The particle's colours carry (strength, noise scale, rise
// speed, 1): the strength is interpolated over the life, faded in over the first 15 % and
// scaled by the allied-VFX opacity for allied effects.

#include "vfx.glsl"
#include "distortion.glsl"

layout(location = 0) out vec2 out_uv;
layout(location = 1) out vec3 out_params;
layout(location = 2) out float out_view_depth;
layout(location = 3) flat out float out_seed;
layout(location = 4) flat out float out_age;

const vec2 CORNERS[6] = vec2[](
    vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
    vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0)
);

void main() {
    uint slot = alive[push.list.x + uint(gl_InstanceIndex)];
    Particle p = particles[slot];
    vec2 corner = CORNERS[gl_VertexIndex % 6];
    float t = clamp(p.pos_age.w / max(p.vel_life.w, 1e-4), 0.0, 1.0);
    float size = mix(p.size_spin.x, p.size_spin.y, t);
    // Never rotated: the noise rises up the screen like hot air.
    vec3 world = p.pos_age.xyz
        + (frame.camera_right.xyz * corner.x + frame.camera_up.xyz * corner.y) * size * 0.5;
    vec4 clip = frame.view_proj * vec4(world, 1.0);
    gl_Position = clip;
    vec4 color = vfx_color(p, t);
    float strength = color.r * smoothstep(0.0, 0.15, t);
    if (((p.misc.x >> VFX_ROLE_SHIFT) & 3u) == VFX_ROLE_ALLY) {
        strength *= frame.depth.z;
    }
    out_uv = corner * 0.5 + 0.5;
    out_params = vec3(strength, color.g, color.b);
    out_view_depth = clip.w;
    // A per-particle noise offset, so neighbouring hazes do not shimmer in step.
    out_seed = float(p.misc.w % 4096u) * 0.618034;
    out_age = p.pos_age.w;
}
