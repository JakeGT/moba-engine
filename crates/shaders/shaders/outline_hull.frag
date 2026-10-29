#version 450

// Inverted-hull outline colour (Low and Medium, ADR-0016): the slot's palette colour, unlit.
// It goes through the tonemap with the scene.

layout(location = 0) flat in vec4 in_color;

layout(location = 0) out vec4 out_color;

void main() {
    out_color = in_color;
}
