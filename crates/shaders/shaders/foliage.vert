#version 450
#extension GL_GOOGLE_include_directive : require

// Procedural foliage (R3b, ADR-0017 §2). No vertex buffer: each instance is a clump of up
// to 8 blades, each blade two segments (9 vertices, 3 triangles), built from gl_VertexIndex.
// Kinds: 0 grass, 1 brush (broad leaves leaning out), 2 reeds (tall, thin, upright),
// 3 flowers (grass with a coloured head on the first blade). Wind is two sine gusts along the
// map's wind direction plus a per-instance phase, scaled by the square of the height along
// the blade. Displacers (only units the client draws) push the tips away and flatten them.
// Far clumps drop blades and shrink to nothing at the draw distance, so there is no pop.

#include "camera.glsl"
#include "environment.glsl"

// Set 2 binding 1: the map's instances (xyz position bits, packed yaw/height/kind/tint).
layout(std430, set = 2, binding = 1) readonly buffer FoliageInstances {
    uvec4 instances[];
} foliage;

layout(location = 0) out vec3 out_world_pos;
layout(location = 1) out vec3 out_normal;
layout(location = 2) out float out_height; // 0 at the base, 1 at the tip
layout(location = 3) out vec3 out_albedo;

float hash(uint n) {
    n = (n << 13u) ^ n;
    n = n * (n * n * 15731u + 789221u) + 1376312589u;
    return float(n & 0x7fffffffu) / 2147483647.0;
}

// (side, t) of each of a blade's 9 vertices: base-left, base-right, mid-left; mid-left,
// base-right, mid-right; mid-left, mid-right, tip.
const vec2 BLADE[9] = vec2[9](
    vec2(-1.0, 0.0), vec2(1.0, 0.0), vec2(-1.0, 0.5),
    vec2(-1.0, 0.5), vec2(1.0, 0.0), vec2(1.0, 0.5),
    vec2(-1.0, 0.5), vec2(1.0, 0.5), vec2(0.0, 1.0)
);

void main() {
    uvec4 record = foliage.instances[gl_InstanceIndex];
    vec3 root = uintBitsToFloat(record.xyz);
    uint word = record.w;
    float yaw = float(word & 255u) / 256.0 * 6.2831853;
    float height = float((word >> 8u) & 1023u) / 1023.0 * 2.5;
    uint kind = (word >> 18u) & 3u;
    float tint = 0.5 + float((word >> 20u) & 255u) / 255.0;

    uint blade = uint(gl_VertexIndex) / 9u;
    vec2 corner = BLADE[uint(gl_VertexIndex) % 9u];

    // Distance LOD: fewer blades far away, and the whole clump shrinks over the last 20 %.
    // Measured from the eye or the camera's focus on the ground, whichever is nearer, as the
    // CPU chunk culling does (`visible_runs`): a high camera keeps foliage to the top of
    // the screen.
    float distance_max = environment.time.w;
    float distance = min(length(camera.camera_pos.xyz - root), length(environment.focus.xyz - root));
    float lod = clamp(distance / distance_max, 0.0, 1.0);
    float blades = environment.time.z;
    float live_blades = max(1.0, floor(blades * (1.0 - 0.65 * lod) + 0.5));
    float shrink = 1.0 - smoothstep(0.8, 1.0, lod);
    if (float(blade) >= live_blades || shrink <= 0.0 || height <= 0.0) {
        // Degenerate: every vertex of the blade at one point outside the view.
        gl_Position = vec4(0.0, 0.0, -1.0, 1.0);
        out_world_pos = root;
        out_normal = vec3(0.0, 1.0, 0.0);
        out_height = 0.0;
        out_albedo = vec3(0.0);
        return;
    }

    uint seed = uint(gl_InstanceIndex) * 8u + blade;
    float r0 = hash(seed * 3u + 1u);
    float r1 = hash(seed * 3u + 2u);
    float r2 = hash(seed * 3u + 3u);

    // Per kind: spread radius, width, lean, stiffness.
    float spread = 0.22;
    float width = 0.045;
    float lean = 0.35;
    float stiffness = 1.0;
    if (kind == 1u) {
        spread = 0.25 * height;
        width = 0.22 * height;
        lean = 0.75;
        stiffness = 0.45;
    } else if (kind == 2u) {
        spread = 0.12;
        width = 0.03;
        lean = 0.12;
        stiffness = 0.6;
    }
    float angle = yaw + float(blade) * 2.39996323 + r0 * 0.6;
    vec2 out_dir = vec2(cos(angle), sin(angle));
    vec2 base_xz = root.xz + out_dir * spread * sqrt(r1);
    float blade_height = height * shrink * mix(0.7, 1.0, r2);

    // Wind: two gusts travelling along the wind direction.
    vec2 wind = environment.wind.xy;
    float wind_strength = length(wind);
    vec2 wind_dir = wind_strength > 1e-4 ? wind / wind_strength : vec2(1.0, 0.0);
    float phase = dot(base_xz, wind_dir) * 0.12 + r0 * 6.2831853;
    float t_seconds = environment.time.x * environment.time.y;
    float gust = 0.55 + 0.35 * sin(t_seconds * 2.1 + phase) + 0.15 * sin(t_seconds * 5.3 + phase * 1.7);
    vec2 bend = wind * gust * 0.18 * stiffness;

    // Displacement by nearby drawn units.
    float flatten = 0.0;
    int count = int(environment.wind.z);
    for (int i = 0; i < MOBA_FOLIAGE_DISPLACERS; i++) {
        if (i >= count) {
            break;
        }
        vec4 displacer = environment.displacers[i];
        vec2 away = base_xz - displacer.xy;
        float d = length(away);
        if (d < displacer.z && displacer.z > 0.0) {
            float f = 1.0 - d / displacer.z;
            f = f * f * displacer.w;
            bend += (d > 1e-3 ? away / d : out_dir) * f * 1.2;
            flatten = max(flatten, f);
        }
    }
    blade_height *= 1.0 - 0.55 * flatten;

    float t = corner.y;
    vec2 lean_offset = out_dir * lean * blade_height + bend * blade_height;
    // Keep the blade's length roughly constant as it bends.
    float bent = clamp(length(lean_offset) / max(blade_height, 1e-3), 0.0, 0.95);
    float up = blade_height * sqrt(1.0 - bent * bent);
    vec3 spine = vec3(base_xz.x, root.y, base_xz.y)
        + vec3(lean_offset.x * t * t, up * t, lean_offset.y * t * t);
    vec2 side_dir = vec2(-out_dir.y, out_dir.x);
    float taper = kind == 1u ? (1.0 - t * 0.4) : (1.0 - t * 0.3);
    vec3 position = spine + vec3(side_dir.x, 0.0, side_dir.y) * (corner.x * width * taper);

    // Normal: across the blade, bent towards up for soft, painterly shading.
    vec3 tangent = normalize(vec3(lean_offset.x * 2.0 * t, up, lean_offset.y * 2.0 * t) + vec3(0.0, 1e-3, 0.0));
    vec3 across = vec3(side_dir.x, 0.0, side_dir.y);
    vec3 normal = normalize(cross(across, tangent));
    normal = normalize(mix(normal, vec3(0.0, 1.0, 0.0), 0.55));

    vec3 base_color = environment.base_color.rgb;
    vec3 tip_color = environment.tip_color.rgb;
    if (kind == 1u) {
        tip_color *= vec3(0.75, 0.85, 0.7);
    } else if (kind == 2u) {
        tip_color = mix(tip_color, vec3(0.55, 0.5, 0.3), 0.45);
    }
    vec3 albedo = mix(base_color, tip_color, t) * tint;
    if (kind == 3u && blade == 0u && t > 0.6) {
        albedo = environment.flower_color.rgb * tint;
    }

    out_world_pos = position;
    out_normal = normal;
    out_height = t;
    out_albedo = albedo;
    gl_Position = camera.view_proj * vec4(position, 1.0);
}
