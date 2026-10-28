// Unit lighting shared by every unit permutation: one directional Lambert light, hemispheric
// ambient and a rim term. PBR replaces this in R2; keep the vertex-colour permutation's maths
// unchanged so the golden images stay put.
#ifndef MOBA_LIGHTING_GLSL
#define MOBA_LIGHTING_GLSL

#include "camera.glsl"

// `normal` must be normalised.
vec3 unit_lighting(vec3 albedo, vec3 normal, vec3 world_pos) {
    vec3 light = normalize(camera.light_direction.xyz);
    float diffuse = max(dot(normal, light), 0.0) * camera.light_direction.w;

    // Hemispheric ambient: warmer from above, cooler from below. Cheap and stable.
    vec3 sky = camera.ambient.rgb;
    vec3 ground = camera.ambient.rgb * vec3(0.55, 0.6, 0.7);
    vec3 ambient = mix(ground, sky, normal.y * 0.5 + 0.5);

    // A touch of rim light separates silhouettes from the terrain.
    vec3 view_dir = normalize(camera.camera_pos.xyz - world_pos);
    float rim = pow(1.0 - max(dot(normal, view_dir), 0.0), 3.0) * 0.25;

    return albedo * (ambient + diffuse) + vec3(rim);
}

#endif
