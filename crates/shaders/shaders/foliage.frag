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

#ifdef GBUFFER
// ADR-0023 §1e: the tiered G-buffer prepass. The same surface as the forward shading,
// encoded (gbuffer.glsl) next to the motion; the forward `scene` still shades in N2a.
#include "gbuffer.glsl"
#define MOTION_VECTORS_FRAGMENT
#include "motion.glsl"
layout(location = 14) in vec4 in_motion_current;
layout(location = 15) in vec4 in_motion_previous;
layout(location = 0) out vec4 out_motion;
layout(location = 1) out vec4 out_gb0;
layout(location = 2) out vec4 out_gb1;
layout(location = 3) out vec4 out_gb2;
#else
layout(location = 0) out vec4 out_color;
#endif

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
#ifdef GBUFFER
    out_motion = vec4(motion_pixels(in_motion_current, in_motion_previous), 0.0, 1.0);
    SurfaceParams params;
    params.base_color = surface.albedo;
    params.metallic = surface.metallic;
    params.normal = surface.normal;
    params.roughness = surface.roughness;
    params.model = GBUFFER_MODEL_FOLIAGE;
    // The forward back light below, `0.35 * through * height`, as the model's translucency.
    params.model_data = vec3(0.35 * in_height, 0.0, 0.0);
    params.ao = surface.occlusion;
    params.flags = 0u;
    params.rim_slot = 0u;
    encode_gbuffer(params, out_gb0, out_gb1, out_gb2);
#else
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
    float mark = 0.0; // ADR-0019: telegraph bands in the alpha, for the heat haze
    lit = apply_glow_decals(lit, in_world_pos, aa, mark);
    out_color = vec4(lit, 1.0 - mark);
#endif
}
