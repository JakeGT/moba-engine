// Per-frame camera and lighting block (set 0, binding 0), shared by every scene shader.
//
// Layout must match `CameraUniform` in crates/render/src/scene.rs. Shaders that ignore fog of
// war or lighting still declare the whole block: one declaration, one layout.
#ifndef MOBA_CAMERA_GLSL
#define MOBA_CAMERA_GLSL

#define MOBA_VISION_CIRCLES 16
#define MOBA_VISION_WALLS 32

// `shadow_params.x`: the frame's shadow filter (ADR-0011).
#define MOBA_SHADOW_OFF 0.0
#define MOBA_SHADOW_HARD 1.0
#define MOBA_SHADOW_PCF 2.0
#define MOBA_SHADOW_PCSS 3.0

// Outline palette slots (R2b, ADR-0016): slot 0 is "no outline".
#define MOBA_OUTLINE_SLOTS 8

layout(set = 0, binding = 0) uniform CameraBlock {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 light_direction; // xyz direction *towards* the light, w = intensity
    vec4 ambient;         // rgb hemispheric sky ambient (the fallback without an environment)
    vec4 terrain;         // rgb tint, w grid-line strength (1 = default)
    vec4 vision_circles[MOBA_VISION_CIRCLES];
    uvec4 vision_count;
    vec4 vision_walls[MOBA_VISION_WALLS];
    uvec4 wall_count;
    // R2a lighting (ADR-0011).
    mat4 light_view_proj;  // world -> shadow-map clip space (xy -1..1, z 0..1)
    vec4 sun_color;        // rgb key-light colour (times light_direction.w), w = shadow strength
    vec4 shadow_params;    // x filter (MOBA_SHADOW_*), y 1 / resolution, z PCSS penumbra texels
                           // per unit of depth difference, w normal-offset bias (world units)
    vec4 shadow_extra;     // x PCF/PCSS taps, y blocker-search radius (texels), z max penumbra
                           // radius (texels), w reserved
    vec4 ibl_params;       // x intensity (0 = hemispheric ambient), y specular max LOD,
                           // z 1 = sample the specular cube, w reserved
    vec4 style;            // x wrap, y rim strength, z ramp strength, w vertex-colour roughness
    vec4 rim_color;        // rgb rim colour, w terrain roughness
    vec4 specular_tint;    // rgb painted-specular tint, w specular scale
    vec4 ambient_ground;   // rgb hemispheric ground ambient, w reserved
    vec4 sh[9];            // SH-L2 diffuse irradiance / pi (rgb), environment maps only
    // R2b (ADR-0016): screen reconstruction, dynamic lights, occlusion and outlines.
    mat4 inv_view_proj;    // clip -> world (contact shadows)
    vec4 viewport;         // xy extent in pixels, zw 1 / extent
    vec4 light_grid;       // xy grid minimum (world x, z), zw 1 / cell size (x, z)
    uvec4 light_info;      // xy grid columns (x, z), z words per column, w lights (0 = none)
    vec4 occlusion;        // x baked AO strength (0 = off), y 1 = contact shadows bound,
                           // z SSAO strength (R6; 0 = none), w reserved
    vec4 baked_ao;         // xy AO map origin (world x, z), z 1 / map size, w height falloff
    vec4 contact;          // x ray steps, y ray length, z thickness, w fade-out distance
    vec4 outline_palette[MOBA_OUTLINE_SLOTS]; // rgb linear display colour, w width (pixels)
} camera;

#endif
