#version 450

// Fog-of-war raster (ADR-0014): one instanced quad per vision source, covering its disc in
// the visibility texture. No vertex buffer: six vertices per instance, two triangles. The
// texture's texel (0, 0) is at the world corner `push.rect.xy`; NDC follows directly (no
// viewport flip), so texture row j is world z = rect.y + (j + 0.5) / rect.z / resolution.

layout(set = 0, binding = 0) readonly buffer SourceBlock {
    vec4 sources[]; // xy world x, z; z radius; w reserved
} source_data;

layout(push_constant) uniform FogPush {
    vec4 rect;   // xy world corner, z 1 / size, w texel size (world units)
    uvec4 counts; // x walls, y sources
} push;

layout(location = 0) out vec2 out_world;
layout(location = 1) flat out vec3 out_source;

void main() {
    vec4 source = source_data.sources[gl_InstanceIndex];
    const vec2 corners[6] = vec2[6](
        vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0),
        vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    // Pad by a texel so the disc's rim is never clipped by the quad.
    float extent = source.z + push.rect.w;
    vec2 world = source.xy + corners[gl_VertexIndex] * extent;
    vec2 uv = (world - push.rect.xy) * push.rect.z;
    gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
    out_world = world;
    out_source = source.xyz;
}
