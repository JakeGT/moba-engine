#version 450

// Depth pre-pass ID output (R2b, ADR-0016): the instance's outline palette slot into the
// R8_UINT ID target the screen-space outline pass dilates. Depth is the fixed-function part.
// A unit without an outline (slot 0) writes 8 (`UNIT_ID_PLAIN`, ADR-0019), so the ID target
// also says where every unit is (the heat haze masks them); the terrain writes nothing (0)
// and the outline pass treats 8 like 0.

layout(location = 0) flat in uint in_outline;

layout(location = 0) out uint out_id;

void main() {
    out_id = in_outline == 0u ? 8u : in_outline;
}
