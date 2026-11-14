#version 450
#extension GL_GOOGLE_include_directive : require

// Motion vectors of the temporal prepass (ADR-0021 §2b), attachment 0 (`R16G16_SFLOAT`).
// `PREPASS_IDS` (units, when distortion keeps its unit mask) also writes the outline slot
// into attachment 1 as `prepass.frag` does; the terrain and the foliage draw without it and
// leave the ID target's clear (the pipeline masks attachment 1).

#include "camera.glsl"
#define MOTION_VECTORS_FRAGMENT
#include "motion.glsl"

layout(location = 14) in vec4 in_motion_current;
layout(location = 15) in vec4 in_motion_previous;
#ifdef PREPASS_IDS
layout(location = 0) flat in uint in_outline;
#endif

layout(location = 0) out vec4 out_motion;
#ifdef PREPASS_IDS
layout(location = 1) out uint out_id;
#endif

void main() {
    out_motion = vec4(motion_pixels(in_motion_current, in_motion_previous), 0.0, 1.0);
#ifdef PREPASS_IDS
    out_id = in_outline == 0u ? 8u : in_outline;
#endif
}
