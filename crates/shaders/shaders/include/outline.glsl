// Inverted-hull team/selection outlines (Low and Medium, ADR-0016). Shared by the
// `OUTLINE_HULL` permutations of unit.vert and skinned.vert; needs camera.glsl.
#ifndef MOBA_OUTLINE_GLSL
#define MOBA_OUTLINE_GLSL

// A clip position outside the view volume: instances without an outline collapse onto it,
// so one instanced draw per mesh serves every unit without CPU filtering.
const vec4 MOBA_OUTLINE_NONE = vec4(2.0, 2.0, 2.0, 1.0);
// The hull sits this far behind its own surface along the view ray, so the unit in front
// hides it everywhere but the silhouette ring.
const float MOBA_OUTLINE_PUSH_BACK = 0.3;

// Hull vertex: `world` pushed back along the view ray, then offset along the screen-space
// direction of `extrude` (a world vector) by the slot's width in pixels.
vec4 outline_hull_position(vec3 world, vec3 extrude, uint slot) {
    if (slot == 0u || slot >= uint(MOBA_OUTLINE_SLOTS)) {
        return MOBA_OUTLINE_NONE;
    }
    float width = camera.outline_palette[slot].w;
    if (width <= 0.0) {
        return MOBA_OUTLINE_NONE;
    }
    vec3 view_ray = world - camera.camera_pos.xyz;
    float ray_length = length(view_ray);
    vec3 pushed = ray_length > 1e-4 ? world + view_ray / ray_length * MOBA_OUTLINE_PUSH_BACK : world;
    vec4 clip = camera.view_proj * vec4(pushed, 1.0);
    vec2 screen = (camera.view_proj * vec4(extrude, 0.0)).xy;
    float screen_length = length(screen);
    if (screen_length > 1e-6) {
        // Pixels to NDC: two units span the viewport; times w so the offset survives the
        // perspective divide.
        clip.xy += screen / screen_length * width * 2.0 * camera.viewport.zw * clip.w;
    }
    return clip;
}

// The slot's colour (linear display RGB), opaque.
vec4 outline_color(uint slot) {
    return vec4(camera.outline_palette[min(slot, uint(MOBA_OUTLINE_SLOTS) - 1u)].rgb, 1.0);
}

#endif
