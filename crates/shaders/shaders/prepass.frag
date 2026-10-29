#version 450

// Depth pre-pass ID output (R2b, ADR-0016): the instance's outline palette slot into the
// R8_UINT ID target the screen-space outline pass dilates. Depth is the fixed-function part.

layout(location = 0) flat in uint in_outline;

layout(location = 0) out uint out_id;

void main() {
    out_id = in_outline;
}
