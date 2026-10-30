#version 450
#extension GL_GOOGLE_include_directive : require

// Sky backdrop (ADR-0014): a fullscreen triangle on the far plane (depth 1), drawn after the
// opaque scene with LESS_OR_EQUAL so it only covers pixels nothing else wrote. The view ray
// is unprojected per vertex: `far.xyz - eye * far.w` is linear in clip space, so the
// rasteriser interpolates it exactly and the fragment shader only normalises it.

#include "camera.glsl"

layout(location = 0) out vec3 out_ray;

void main() {
    vec2 corner = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    vec2 ndc = corner * 2.0 - 1.0;
    gl_Position = vec4(ndc, 1.0, 1.0);
    vec4 far = inverse(camera.view_proj) * vec4(ndc, 1.0, 1.0);
    out_ray = far.xyz - camera.camera_pos.xyz * far.w;
}
