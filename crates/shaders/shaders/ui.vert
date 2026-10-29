#version 450

// Matches `UiPush` in crates/render/src/passes/overlay.rs.
layout(push_constant) uniform UiPush {
    vec2 viewport;
    float font_range_px;
    float pad;
} push;

layout(location = 0) in vec2 in_position;
layout(location = 1) in vec2 in_uv;
layout(location = 2) in uint in_mode;
layout(location = 3) in vec4 in_color;
layout(location = 4) in uint in_outline;
layout(location = 5) in vec2 in_params;

layout(location = 0) out vec2 out_uv;
layout(location = 1) out vec4 out_color;
layout(location = 2) flat out uint out_mode;
layout(location = 3) flat out vec4 out_outline;
layout(location = 4) flat out vec2 out_params;

void main() {
    vec2 ndc = vec2(
        in_position.x / push.viewport.x * 2.0 - 1.0,
        1.0 - in_position.y / push.viewport.y * 2.0
    );
    gl_Position = vec4(ndc, 0.0, 1.0);
    out_uv = in_uv;
    out_color = in_color;
    out_mode = in_mode;
    out_outline = unpackUnorm4x8(in_outline);
    out_params = in_params;
}
