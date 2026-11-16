#version 450
#extension GL_GOOGLE_include_directive : require

// The temporal prepass's background motion (ADR-0021 §2b): a fullscreen draw at the far plane
// (depth-tested LESS_OR_EQUAL, so only where nothing was drawn) whose view direction, through
// the unjittered and the previous matrix as a direction (w = 0), gives the camera's rotation
// only. `sky.vert` supplies the per-pixel ray from the jittered inverse. The G-buffer is
// written as zero (`UNLIT`, no flags, ADR-0023 §1e) and, with `PREPASS_IDS`, the ID as 0 (no
// unit): written explicitly with RGBA masks, since lavapipe does not honour an empty mask
// over an unwritten output (amendment 1 item 16). Both equal the attachments' clears.

#include "camera.glsl"
#ifdef GBUFFER_HDR
#include "sky_radiance.glsl"
#endif
#define MOTION_VECTORS_FRAGMENT
#include "motion.glsl"

layout(location = 0) in vec3 in_ray;

layout(location = 0) out vec4 out_motion;
layout(location = 1) out vec4 out_gb0;
layout(location = 2) out vec4 out_gb1;
layout(location = 3) out vec4 out_gb2;
#ifdef PREPASS_IDS
layout(location = 4) out uint out_id;
#endif
#ifdef GBUFFER_HDR
// N2b (ADR-0023 §1a, §2, amendment 2 items 6 and 7): the light-accumulation target, after the
// outline IDs when they are attached: the sky's radiance, which the deferred lighting never
// overwrites (it skips the far plane). Without the world's sky (`SKY_RADIANCE` false) the
// shader writes the main view's clear colour itself, the forward frame's background. It is
// not left to blending or an empty mask, which lavapipe does not honour (amendment 1 item
// 16); `hdr` is not cleared, so this draw is the only writer of every background pixel.
layout(constant_id = 0) const bool SKY_RADIANCE = true;
layout(constant_id = 1) const float CLEAR_R = 0.0;
layout(constant_id = 2) const float CLEAR_G = 0.0;
layout(constant_id = 3) const float CLEAR_B = 0.0;
layout(constant_id = 4) const float CLEAR_A = 1.0;
#ifdef PREPASS_IDS
layout(location = 5) out vec4 out_hdr;
#else
layout(location = 4) out vec4 out_hdr;
#endif
#endif

void main() {
    vec4 current = camera.view_proj_unjittered * vec4(in_ray, 0.0);
    vec4 previous = camera.prev_view_proj * vec4(in_ray, 0.0);
    vec2 motion = vec2(0.0);
    if (current.w > 1e-6 && previous.w > 1e-6) {
        motion = motion_pixels(current, previous);
    }
    out_motion = vec4(motion, 0.0, 1.0);
    out_gb0 = vec4(0.0);
    out_gb1 = vec4(0.0);
    out_gb2 = vec4(0.0);
#ifdef PREPASS_IDS
    out_id = 0u;
#endif
#ifdef GBUFFER_HDR
    out_hdr = SKY_RADIANCE ? vec4(sky_radiance(in_ray), 1.0)
                           : vec4(CLEAR_R, CLEAR_G, CLEAR_B, CLEAR_A);
#endif
}
