// The environment block shared by the foliage and water pipelines (R3b, ADR-0017): set 2
// binding 0 of both layouts (`EnvironmentBlock` in crates/render/src/environment.rs, std140,
// vec4s only). One per frame in flight.
#ifndef MOBA_ENVIRONMENT_GLSL
#define MOBA_ENVIRONMENT_GLSL

#define MOBA_FOLIAGE_DISPLACERS 32
#define MOBA_WATER_BODIES 8

layout(set = 2, binding = 0) uniform EnvironmentBlock {
    mat4 inverse_view_proj;
    vec4 time;          // x seconds, y gust speed, z blades per clump, w draw distance
    vec4 wind;          // xy wind (world x, z), z displacers, w water normal octaves
    vec4 base_color;    // foliage base (rgb)
    vec4 tip_color;     // foliage tips (rgb)
    vec4 flower_color;  // flower heads (rgb)
    vec4 screen;        // xy 1 / target size, z refraction strength, w 1 = full water
    vec4 focus;         // xyz the camera's focus on the ground (foliage distance)
    vec4 displacers[MOBA_FOLIAGE_DISPLACERS]; // xz, radius, strength
    vec4 water_deep[MOBA_WATER_BODIES];       // rgb deep colour, w clarity (m)
    vec4 water_shallow[MOBA_WATER_BODIES];    // rgb shallow colour, w foam
} environment;

#endif
