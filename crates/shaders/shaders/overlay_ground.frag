#version 450

// The overlay prepass's ground class (ADR-0021 §5c): terrain and water surfaces write 9, so
// the overlay draws telegraphs on them and nowhere else. Units draw with `prepass.frag` (their
// outline slot, or 8, `OCCLUDER`); 0 is the sky.

layout(location = 0) out uint out_id;

void main() {
    out_id = 9u;
}
