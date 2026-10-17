#version 450

layout(push_constant) uniform UiPush {
    vec2 viewport;
} push;

layout(location = 0) in vec2 in_position;
layout(location = 1) in vec4 in_color;

layout(location = 0) out vec4 out_color;

void main() {
    vec2 ndc = vec2(
        in_position.x / push.viewport.x * 2.0 - 1.0,
        1.0 - in_position.y / push.viewport.y * 2.0
    );
    gl_Position = vec4(ndc, 0.0, 1.0);
    out_color = in_color;
}
