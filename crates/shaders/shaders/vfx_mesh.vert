#version 450
#extension GL_GOOGLE_include_directive : require

// GPU mesh particles: a registered mesh per particle from one mesh slot's compacted list,
// tumbling about a per-particle axis, lit by the key light.

#include "vfx.glsl"

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;

layout(push_constant) uniform Push {
    uint mode;
    uint slot;
} push;

layout(location = 0) out vec2 out_uv;
layout(location = 1) out vec4 out_color;
layout(location = 2) out float out_view_depth;
layout(location = 3) flat out uint out_flags;
#ifdef VFX_TEMPORAL
// ADR-0021 §2d: the previous position inverts the simulation's step (`position - velocity *
// dt`), the tumble at `age - dt`.
layout(location = 14) out vec4 out_motion_current;
layout(location = 15) out vec4 out_motion_previous;
#endif

vec3 rotate(vec3 v, vec3 axis, float angle) {
    float c = cos(angle);
    float s = sin(angle);
    return v * c + cross(axis, v) * s + axis * dot(axis, v) * (1.0 - c);
}

void main() {
    uint capacity = frame.counts.x;
    uint id = alive[capacity + min(push.slot, 3u) * frame.counts.z + gl_InstanceIndex];
    Particle p = particles[id];
    float t = clamp(p.pos_age.w / max(p.vel_life.w, 1e-4), 0.0, 1.0);
    float size = mix(p.size_spin.x, p.size_spin.y, t);
    float angle = p.seg_rot.w + p.size_spin.z * p.pos_age.w;
    // A stable per-slot tumble axis.
    float h = fract(sin(float(id) * 12.9898) * 43758.5453);
    vec3 axis = normalize(vec3(h - 0.5, 1.0, fract(h * 7.31) - 0.5));
    vec3 local = rotate(in_position * size, axis, angle);
    vec3 normal = normalize(rotate(in_normal, axis, angle));
    vec4 clip = frame.view_proj * vec4(p.pos_age.xyz + local, 1.0);
    gl_Position = clip;
    vec4 color = vfx_legible(vfx_color(p, t), p.misc.x, t);
    float lit = frame.light.w + (1.0 - frame.light.w) * max(dot(normal, frame.light.xyz), 0.0);
    color.rgb *= lit;
    out_color = color;
    out_uv = vfx_atlas_uv(p, vec2(0.5), t);
    out_view_depth = clip.w;
    out_flags = p.misc.x | VFX_FLAG_HARD;
#ifdef VFX_TEMPORAL
    float dt = frame.camera_right.w;
    float prev_age = max(p.pos_age.w - dt, 0.0);
    float prev_t = clamp(prev_age / max(p.vel_life.w, 1e-4), 0.0, 1.0);
    float prev_size = mix(p.size_spin.x, p.size_spin.y, prev_t);
    float prev_angle = p.seg_rot.w + p.size_spin.z * prev_age;
    vec3 prev_local = rotate(in_position * prev_size, axis, prev_angle);
    vec3 world = p.pos_age.xyz + local;
    vec3 previous = p.pos_age.xyz - p.vel_life.xyz * dt + prev_local;
    out_motion_current = frame.view_proj_unjittered * vec4(world, 1.0);
    out_motion_previous = frame.prev_view_proj * vec4(previous, 1.0);
#endif
}
