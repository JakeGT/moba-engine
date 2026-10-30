#version 450
#extension GL_GOOGLE_include_directive : require

// Foliage shading (R3b, ADR-0017 §2): `shade()` on a base-to-tip gradient with a soft,
// up-bent normal, a little back-light translucency towards the tips, the occlusion of the
// clump's base, the fog of war, height fog and both decal styles (paint into the albedo, glow
// over the result), like the terrain it grows from.

#include "camera.glsl"
#include "lighting.glsl"
#include "atmosphere.glsl"
#include "decals.glsl"

layout(location = 0) in vec3 in_world_pos;
layout(location = 1) in vec3 in_normal;
layout(location = 2) in float in_height;
layout(location = 3) in vec3 in_albedo;

layout(location = 0) out vec4 out_color;

void main() {
    float aa = decal_pixel(in_world_pos);
    vec3 normal = normalize(in_normal);
    if (!gl_FrontFacing) {
        normal = normalize(vec3(-normal.x, normal.y, -normal.z));
    }
    // The clump's base sits in its own shade.
    float occlusion = mix(0.55, 1.0, smoothstep(0.0, 0.7, in_height));
    vec3 albedo = apply_paint_decals(in_albedo * occlusion, in_world_pos, aa);
    Surface surface = default_surface(albedo, normal, in_world_pos, 0.8);
    surface.geometric_normal = vec3(0.0, 1.0, 0.0);
    surface.rim = 0.0;
    vec3 lit = shade(surface);

    // Translucency: sunlight through the blade when looking towards the sun.
    vec3 view = normalize(camera.camera_pos.xyz - in_world_pos);
    vec3 light = normalize(camera.light_direction.xyz);
    float through = pow(clamp(dot(-view, light), 0.0, 1.0), 3.0) * in_height;
    lit += albedo * camera.sun_color.rgb * (0.35 * through);

    lit = apply_fog_of_war(lit, in_world_pos);
    if (camera.haze_params.w > 0.5) {
        float distance = length(camera.camera_pos.xyz - in_world_pos);
        lit = mix(lit, camera.ambient.rgb * 1.2, smoothstep(240.0, 520.0, distance));
    } else {
        lit = apply_height_fog(lit, in_world_pos);
    }
    lit = apply_glow_decals(lit, in_world_pos, aa);
    out_color = vec4(lit, 1.0);
}
