#version 450

// The transparency-and-composition mask (ADR-0021 §2d): water surfaces, drawn with
// `water.vert` and depth-tested against the scene, write 1 (MAX blended). Their shading
// moves every frame (flow-scrolled normals, refraction) without moving the geometry, so the
// upscalers narrow their history clip there. No material is flagged `animated_uv` yet; one
// would draw here too.

layout(location = 0) out vec4 out_mask;

void main() {
    out_mask = vec4(1.0, 0.0, 0.0, 0.0);
}
