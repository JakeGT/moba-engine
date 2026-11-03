// Motion vectors (ADR-0021 §2): the prepass's `MOTION_VECTORS` permutations write the
// unjittered current and the previous clip position of each vertex; `prepass_motion.frag`
// divides per fragment, so the interpolation is exact on every triangle (perspective-correct
// clip positions), and writes render pixels from the current sample to its previous position,
// x right and y down (the scene draws through a flipped viewport: NDC +y is the top), without
// jitter. Includers declare the camera block first.
//
// The previous-instance buffer (set 1, binding 2) is indexed like the instances: the previous
// model matrix as three rows and the previous VAT pose (`PrevInstance` in
// crates/render/src/passes/scene.rs). On Compat it aliases the instance buffer and no shader
// reads it.
#ifndef MOBA_MOTION_GLSL
#define MOBA_MOTION_GLSL

// `PrevInstance::pose.w` bit: this instance has a previous state (otherwise it is drawn where
// it was: camera-only motion).
#define MOBA_PREV_VALID 0x10000u

#ifdef MOTION_PREV_INSTANCES
struct PrevInstance {
    vec4 rows[3];  // previous model matrix, rows 0..2 (the fourth is 0, 0, 0, 1)
    uvec4 pose;    // previous VAT pose: x from frame | next << 16, y to frame | next << 16,
                   // z packUnorm2x16(from t, to t), w packUnorm2x16(blend) low 16 bits |
                   // MOBA_PREV_VALID
};

layout(std430, set = 1, binding = 2) readonly buffer PrevInstanceBlock {
    PrevInstance prev_instances[];
} prev_instance_data;

bool prev_valid(uint instance) {
    return (prev_instance_data.prev_instances[instance].pose.w & MOBA_PREV_VALID) != 0u;
}

mat4 prev_model(uint instance) {
    vec4 r0 = prev_instance_data.prev_instances[instance].rows[0];
    vec4 r1 = prev_instance_data.prev_instances[instance].rows[1];
    vec4 r2 = prev_instance_data.prev_instances[instance].rows[2];
    return mat4(
        vec4(r0.x, r1.x, r2.x, 0.0),
        vec4(r0.y, r1.y, r2.y, 0.0),
        vec4(r0.z, r1.z, r2.z, 0.0),
        vec4(r0.w, r1.w, r2.w, 1.0));
}
#endif

#ifdef MOTION_VECTORS_VERTEX
layout(location = 14) out vec4 out_motion_current;
layout(location = 15) out vec4 out_motion_previous;

// Writes the motion varyings of a vertex at `world` that was at `previous` last frame.
void write_motion(vec3 world, vec3 previous) {
    out_motion_current = camera.view_proj_unjittered * vec4(world, 1.0);
    out_motion_previous = camera.prev_view_proj * vec4(previous, 1.0);
}
#endif

#ifdef MOTION_VECTORS_FRAGMENT
// Render pixels from the current (unjittered) position to the previous one.
vec2 motion_pixels(vec4 current, vec4 previous) {
    vec2 ndc_current = current.xy / current.w;
    vec2 ndc_previous = previous.xy / previous.w;
    return (ndc_previous - ndc_current) * vec2(0.5, -0.5) * camera.temporal.yz;
}
#endif

#endif
