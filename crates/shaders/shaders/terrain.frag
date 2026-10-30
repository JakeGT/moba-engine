#version 450
#extension GL_GOOGLE_include_directive : require

// Terrain shading (ADR-0011 lighting, ADR-0014 terrain).
//
// Splat mode (`terrain_block.params.x` = layers sampled > 0): up to eight layers from two
// texture arrays, weighted by a two-layer RGBA splat array over the terrain square. Each
// layer's albedo is tinted, its alpha (ambient occlusion / cavity) sharpens the blend and
// darkens crevices; top-down projection on flat ground and triplanar on slopes (High and
// up); the layers' normal maps (Medium and up) with a UDN blend. Texture fetches inside the
// per-layer branches use explicit gradients, so skipping zero-weight layers stays valid.
// Flat mode: the R2a plane's procedural grid and the map tint. Both take the R3b decals
// (ADR-0017): paint into the albedo, glow over the final colour.

#include "camera.glsl"
#include "lighting.glsl"
#include "atmosphere.glsl"
#include "decals.glsl"

layout(set = 3, binding = 6) uniform sampler2DArray terrain_splat;
layout(set = 3, binding = 7) uniform sampler2DArray terrain_albedo;
layout(set = 3, binding = 8) uniform sampler2DArray terrain_normal;
layout(set = 3, binding = 9) uniform TerrainBlock {
    vec4 rect;      // xy world x, z of the splat square's corner, zw 1 / size
    vec4 params;    // x layers sampled (0 = flat), y normal maps, z triplanar, w normal strength
    vec4 splat;     // x layers of the splat array (1 or 2), yzw reserved
    vec4 tiling[2]; // 1 / metres per repeat, per layer
    vec4 tint[8];   // rgb layer tint
} terrain_block;

layout(location = 0) in vec3 in_normal;
layout(location = 1) in vec3 in_world_pos;
layout(location = 2) in float in_occlusion;

layout(location = 0) out vec4 out_color;

// World-space pixel size for the decal edges (set in main, in uniform control flow).
float decal_aa;

float grid_line(vec2 position, float spacing, float thickness) {
    vec2 coord = position / spacing;
    vec2 derivative = fwidth(coord);
    vec2 grid = abs(fract(coord - 0.5) - 0.5) / max(derivative, vec2(1e-5));
    return 1.0 - min(min(grid.x, grid.y) / thickness, 1.0);
}

float layer_tiling(int layer) {
    return terrain_block.tiling[layer >> 2][layer & 3];
}

vec3 flat_terrain(vec3 normal) {
    // `terrain.w` scales the grid lines: 1 for the world, fainter for the baked minimap (R7b).
    float grid = max(grid_line(in_world_pos.xz, 4.0, 1.0), grid_line(in_world_pos.xz, 16.0, 1.4) * 0.6)
        * camera.terrain.w;
    vec3 grass = camera.terrain.rgb;
    vec3 line = camera.terrain.rgb * 2.0;
    vec3 albedo = apply_paint_decals(mix(grass, line, grid), in_world_pos, decal_aa);
    Surface surface = default_surface(albedo, normal, in_world_pos, camera.rim_color.w);
    surface.rim = 0.0;
    return shade(surface);
}

vec2 decode_normal(vec2 encoded) {
    return (encoded * 2.0 - 1.0) * terrain_block.params.w;
}

vec3 splat_terrain(vec3 geometric) {
    vec2 splat_uv = (in_world_pos.xz - terrain_block.rect.xy) * terrain_block.rect.zw;
    vec4 low = texture(terrain_splat, vec3(splat_uv, 0.0));
    // A single splat map carries layers 1-4 only: sampling layer 1 of a one-layer array
    // would clamp to layer 0 and double the base weights. Dynamically uniform branch.
    vec4 high = vec4(0.0);
    if (terrain_block.splat.x > 1.5) {
        high = texture(terrain_splat, vec3(splat_uv, 1.0));
    }
    float weights[8] = float[8](low.x, low.y, low.z, low.w, high.x, high.y, high.z, high.w);
    int layers = int(terrain_block.params.x);
    // Layers past the tier's budget (or the map's count) fold into the base layer.
    for (int i = 1; i < 8; i++) {
        if (i >= layers) {
            weights[0] += weights[i];
        }
    }
    bool normals = terrain_block.params.y > 0.5;
    // Triplanar only where the ground slopes: flat board pixels pay for one projection.
    bool triplanar = terrain_block.params.z > 0.5 && geometric.y < 0.85;
    vec3 blend = vec3(0.0, 1.0, 0.0);
    if (triplanar) {
        blend = pow(abs(geometric), vec3(4.0));
        blend /= max(dot(blend, vec3(1.0)), 1e-4);
    }
    vec3 dx = dFdx(in_world_pos);
    vec3 dy = dFdy(in_world_pos);

    vec3 albedo = vec3(0.0);
    vec3 normal_sum = vec3(0.0);
    float cavity = 0.0;
    float total = 0.0;
    for (int i = 0; i < 8; i++) {
        if (i >= layers) {
            break;
        }
        float w = weights[i];
        if (w < 0.004) {
            continue;
        }
        float scale = layer_tiling(i);
        float layer = float(i);
        vec3 top = vec3(in_world_pos.xz * scale, layer);
        vec4 colour = textureGrad(terrain_albedo, top, dx.xz * scale, dy.xz * scale) * blend.y;
        vec3 n = vec3(0.0);
        if (normals) {
            vec2 ts = decode_normal(textureGrad(terrain_normal, top, dx.xz * scale, dy.xz * scale).rg);
            n += normalize(vec3(ts.x + geometric.x, geometric.y, ts.y + geometric.z)) * blend.y;
        }
        if (triplanar) {
            vec3 side_x = vec3(in_world_pos.zy * scale, layer);
            vec3 side_z = vec3(in_world_pos.xy * scale, layer);
            colour += textureGrad(terrain_albedo, side_x, dx.zy * scale, dy.zy * scale) * blend.x;
            colour += textureGrad(terrain_albedo, side_z, dx.xy * scale, dy.xy * scale) * blend.z;
            if (normals) {
                vec2 tx = decode_normal(textureGrad(terrain_normal, side_x, dx.zy * scale, dy.zy * scale).rg);
                vec2 tz = decode_normal(textureGrad(terrain_normal, side_z, dx.xy * scale, dy.xy * scale).rg);
                n += normalize(vec3(geometric.x, tx.y + geometric.y, tx.x + geometric.z)) * blend.x;
                n += normalize(vec3(tz.x + geometric.x, tz.y + geometric.y, geometric.z)) * blend.z;
            }
        }
        // Height blend: the layer's cavity (alpha) sharpens the transitions, so grass fills
        // the gaps between pebbles instead of cross-fading.
        float h = w * (0.2 + colour.a * colour.a);
        albedo += colour.rgb * terrain_block.tint[i].rgb * h;
        cavity += colour.a * h;
        normal_sum += n * h;
        total += h;
    }
    total = max(total, 1e-4);
    albedo /= total;
    cavity /= total;
    vec3 normal = geometric;
    if (normals && dot(normal_sum, normal_sum) > 1e-8) {
        normal = normalize(normal_sum);
    }
    // Stylised occlusion: the layer cavity and the baked relief occlusion darken the albedo
    // (the painterly convention; ADR-0011's R2b hook will move it to the ambient term). With
    // R6's GTAO on (High, Ultra) the relief term instead joins the ambient occlusion as
    // min(baked, GTAO): both see the same crevices, and stacking them would darken them twice
    // (ADR-0015 §5). Low and Medium keep the baked term alone. Paint decals (R3b) go into the
    // albedo after it, so they are lit and shadowed like the ground.
    float relief = clamp(in_occlusion, 0.0, 1.0);
    bool gtao = camera.post_params.x > 0.0;
    float occlusion = mix(1.0, cavity * (gtao ? 1.0 : relief), 0.75);
    albedo = apply_paint_decals(albedo * occlusion, in_world_pos, decal_aa);
    Surface surface = default_surface(albedo, normal, in_world_pos, camera.rim_color.w);
    surface.occlusion = gtao ? mix(1.0, relief, 0.75) : 1.0;
    surface.geometric_normal = geometric;
    surface.rim = 0.0;
    return shade(surface);
}

void main() {
    decal_aa = decal_pixel(in_world_pos);
    vec3 geometric = normalize(in_normal);
    vec3 lit = terrain_block.params.x < 0.5 ? flat_terrain(geometric) : splat_terrain(geometric);

    lit = apply_fog_of_war(lit, in_world_pos);

    // Height fog; without it, the R2a distance fade hides the plane's edge.
    if (camera.haze_params.w > 0.5) {
        float distance = length(camera.camera_pos.xyz - in_world_pos);
        lit = mix(lit, camera.ambient.rgb * 1.2, smoothstep(240.0, 520.0, distance));
    } else {
        lit = apply_height_fog(lit, in_world_pos);
    }

    // Telegraphs and zones over everything, fog included (ADR-0017 §1).
    lit = apply_glow_decals(lit, in_world_pos, decal_aa);

    out_color = vec4(lit, 1.0);
}
