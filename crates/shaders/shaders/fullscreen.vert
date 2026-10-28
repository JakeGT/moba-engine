#version 450

// Fullscreen triangle with no vertex buffer: vertices 0, 1, 2 cover clip space
// (-1,-1), (3,-1), (-1,3). Used by the composite/tonemap pass.

void main() {
    vec2 corner = vec2(float((gl_VertexIndex << 1) & 2), float(gl_VertexIndex & 2));
    gl_Position = vec4(corner * 2.0 - 1.0, 0.0, 1.0);
}
