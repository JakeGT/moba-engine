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
// N2b (ADR-0023 §1a, §2): the light-accumulation target, after the outline IDs when they are
// attached. The sky pipeline blends it
// (ZERO, ONE), so the target keeps its clear colour where the sky pass draws nothing.
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
    out_hdr = vec4(0.0);
#endif
}
