// Per-frame camera block (set 0, binding 0), shared by every scene shader.
//
// Layout must match `CameraUniform` in crates/render/src/scene.rs. Shaders that ignore fog of
// war still declare the whole block: one declaration, one layout.
#ifndef MOBA_CAMERA_GLSL
#define MOBA_CAMERA_GLSL

#define MOBA_VISION_CIRCLES 16
#define MOBA_VISION_WALLS 32

layout(set = 0, binding = 0) uniform CameraBlock {
    mat4 view_proj;
    vec4 camera_pos;
    vec4 light_direction; // xyz direction *towards* the light, w = intensity
    vec4 ambient;
    vec4 terrain;
    vec4 vision_circles[MOBA_VISION_CIRCLES];
    uvec4 vision_count;
    vec4 vision_walls[MOBA_VISION_WALLS];
    uvec4 wall_count;
} camera;

#endif
