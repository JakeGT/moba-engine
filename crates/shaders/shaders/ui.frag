#version 450

layout(set = 0, binding = 0) uniform sampler2D ui_atlas;

layout(location = 0) in vec2 in_uv;
layout(location = 1) in vec4 in_color;
layout(location = 2) flat in uint in_atlas;

layout(location = 0) out vec4 out_color;

void main() {
    if (in_atlas != 0u) {
        out_color = in_color;
        return;
    }
    // Explicit LOD avoids undefined derivatives after the divergent branch.
    float alpha = textureLod(ui_atlas, in_uv, 0.0).a;
    out_color = vec4(in_color.rgb, in_color.a * alpha);
}
