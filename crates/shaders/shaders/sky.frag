#version 450
#extension GL_GOOGLE_include_directive : require

// Sky backdrop (ADR-0014): include/sky_radiance.glsl along the view ray.

#include "camera.glsl"
#include "sky_radiance.glsl"

layout(location = 0) in vec3 in_ray;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = vec4(sky_radiance(in_ray), 1.0);
}
