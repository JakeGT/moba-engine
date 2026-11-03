#version 450
#extension GL_GOOGLE_include_directive : require

// The temporal prepass's background motion (ADR-0021 §2b): a fullscreen draw at the far plane
// (depth-tested LESS_OR_EQUAL, so only where nothing was drawn) whose view direction, through
// the unjittered and the previous matrix as a direction (w = 0), gives the camera's rotation
// only. `sky.vert` supplies the per-pixel ray from the jittered inverse.

#include "camera.glsl"
#define MOTION_VECTORS_FRAGMENT
#include "motion.glsl"

layout(location = 0) in vec3 in_ray;

layout(location = 0) out vec4 out_motion;

void main() {
    vec4 current = camera.view_proj_unjittered * vec4(in_ray, 0.0);
    vec4 previous = camera.prev_view_proj * vec4(in_ray, 0.0);
    vec2 motion = vec2(0.0);
    if (current.w > 1e-6 && previous.w > 1e-6) {
        motion = motion_pixels(current, previous);
    }
    out_motion = vec4(motion, 0.0, 1.0);
}
