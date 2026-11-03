// Projected ground decals (R3b, ADR-0017 §1): telegraphs, zones, scorch marks and splats.
//
// Set 3 binding 20 is one storage buffer per frame in flight: up to 128 decal records and a
// 32 x 32 grid over the frame's decals with a 128-bit mask per cell
// (`DecalUpload` in crates/render/src/environment.rs). A receiver (terrain, water, foliage)
// finds its cell and walks the set bits, highest index first, so the enemy telegraphs (the
// lowest indices) are composited last, on top. Shapes are signed distances: no textures.
//
// - `apply_paint_decals` blends paint decals into the albedo before lighting;
// - `apply_glow_decals` composites glow decals over the shaded, fogged colour. Enemy
//   telegraphs keep a two-tone rim: a band brightened to at least 1.8x the ground's
//   luminance and a dark halo inside it, so one of them contrasts with any ground
//   (`glow_reference` is the CPU twin of `decal_coverage` and this composite, unit-tested
//   per shape). Both glow functions also return `mark`, the strongest rim or halo band of a
//   visible telegraph (`MOBA_DECAL_MARK`), which the receivers write as `1 - mark` into the
//   HDR alpha so the heat haze never bends a telegraph's edge (ADR-0019; the CPU twin is
//   `telegraph_mark`).
//
// `aa` is the world-space size of a pixel, computed by the caller in uniform control flow.
#ifndef MOBA_DECALS_GLSL
#define MOBA_DECALS_GLSL

#define MOBA_DECAL_GLOW 1u
#define MOBA_DECAL_ENEMY 2u
#define MOBA_DECAL_PROGRESS 4u
#define MOBA_DECAL_MARK 8u
#define MOBA_DECAL_REACH 4.0

struct DecalRecord {
    vec4 placement; // xz centre, cos and sin of the yaw
    vec4 size;      // radius (length), width, half cone angle, height
    vec4 color;     // linear rgb, opacity (enemy floor applied)
    vec4 params;    // shape + 8 * seed, flag bits, progress, fade
};

// The overlay pass (ADR-0021 §5b) binds the same buffer in its own set.
#ifndef MOBA_DECAL_SET
#define MOBA_DECAL_SET 3
#define MOBA_DECAL_BINDING 20
#endif
layout(std430, set = MOBA_DECAL_SET, binding = MOBA_DECAL_BINDING) readonly buffer DecalBuffer {
    vec4 rect;    // xz grid corner, cells per metre (x, z)
    uvec4 counts; // decals, grid edge, enemy telegraphs, reserved
    DecalRecord decals[128];
    uvec4 cells[1024];
} decal_buffer;

float decal_luma(vec3 color) {
    return dot(color, vec3(0.2126, 0.7152, 0.0722));
}

float decal_hash(float n) {
    return fract(sin(n) * 43758.5453);
}

// World-space size of one pixel at `world`, for antialiased edges. Call in uniform control
// flow (it takes derivatives).
float decal_pixel(vec3 world) {
    return max(length(fwidth(world.xz)), 0.01);
}

// The grid cell of `world`, or -1 outside the grid (or without decals).
int decal_cell(vec3 world) {
    if (decal_buffer.counts.x == 0u) {
        return -1;
    }
    vec2 uv = (world.xz - decal_buffer.rect.xy) * decal_buffer.rect.zw;
    int edge = int(decal_buffer.counts.y);
    if (any(lessThan(uv, vec2(0.0))) || any(greaterThanEqual(uv, vec2(float(edge))))) {
        return -1;
    }
    ivec2 cell = min(ivec2(uv), ivec2(edge - 1));
    return cell.y * edge + cell.x;
}

// Decal `index`'s record. The fields are read from the buffer where they are used, never
// copied into a local `DecalRecord`: the copy kept all sixteen floats live through the shape
// code, which pushed the terrain shader past a register step on NVIDIA and doubled the cost
// of every terrain pixel, decals in view or not (the Ultra overview's scene pass 0.30 ->
// 0.62 ms; perf/0021).
#define MOBA_DECAL(index) decal_buffer.decals[index]

// The signed distance of decal `index`'s shape at `world` (negative inside), its progress
// coordinate (`along`: 0 at the origin, 1 at the far edge) and its rim width. A ring is
// evaluated as the disc out to its band's outer edge. `false` when `world` is out of the
// decal's height reach.
bool decal_shape(uint index, vec3 world, out float sd, out float along, out float rim_width,
                 out float shape) {
    sd = 1e9;
    along = 0.0;
    rim_width = 0.0;
    shape = 0.0;
    if (abs(world.y - MOBA_DECAL(index).size.w) > MOBA_DECAL_REACH) {
        return false;
    }
    vec2 offset = world.xz - MOBA_DECAL(index).placement.xy;
    // Into the decal's frame: +x along the yaw.
    vec2 axis = MOBA_DECAL(index).placement.zw;
    vec2 p = vec2(dot(offset, axis), dot(offset, vec2(-axis.y, axis.x)));
    shape = mod(MOBA_DECAL(index).params.x, 8.0);
    float seed = floor(MOBA_DECAL(index).params.x / 8.0);
    float radius = MOBA_DECAL(index).size.x;
    rim_width = clamp(radius * 0.08, 0.12, 0.35);
    if (shape < 0.5) {
        sd = length(p) - radius;
        along = length(p) / radius;
    } else if (shape < 1.5) {
        rim_width = MOBA_DECAL(index).size.y;
        sd = length(p) - (radius + 0.5 * rim_width);
        along = length(p) / radius;
    } else if (shape < 2.5) {
        // Sector about +x with half angle size.z (iq's sdPie with the axis swapped).
        vec2 q = vec2(abs(p.y), p.x);
        vec2 c = vec2(sin(MOBA_DECAL(index).size.z), cos(MOBA_DECAL(index).size.z));
        float l = length(q) - radius;
        float m = length(q - c * clamp(dot(q, c), 0.0, radius));
        sd = max(l, m * sign(c.y * q.x - c.x * q.y));
        along = length(p) / radius;
    } else if (shape < 3.5) {
        vec2 half_size = vec2(radius * 0.5, MOBA_DECAL(index).size.y * 0.5);
        vec2 b = abs(p - vec2(half_size.x, 0.0)) - half_size;
        sd = length(max(b, 0.0)) + min(max(b.x, b.y), 0.0);
        rim_width = clamp(MOBA_DECAL(index).size.y * 0.15, 0.08, 0.3);
        along = clamp(p.x / max(radius, 1e-3), 0.0, 1.0);
    } else {
        // Splat: a blot whose radius wobbles with the angle, plus a few droplets.
        float angle = atan(p.y, p.x);
        float wobble = 0.78 + 0.12 * sin(5.0 * angle + seed) + 0.07 * sin(11.0 * angle + seed * 1.7)
            + 0.04 * sin(23.0 * angle + seed * 3.1);
        sd = length(p) - radius * wobble;
        for (int i = 0; i < 3; i++) {
            float a = decal_hash(seed + float(i) * 7.0) * 6.2831853;
            float r = radius * (1.05 + 0.25 * decal_hash(seed + float(i) * 13.0));
            float s = radius * (0.08 + 0.08 * decal_hash(seed + float(i) * 19.0));
            sd = min(sd, length(p - vec2(cos(a), sin(a)) * r) - s);
        }
        along = 1.0;
    }
    return true;
}

// Coverage of decal `index` at `world`: x the washed area, y the rim band, z the progress
// fill, w the halo band just inside the rim. Zero outside. A ring is evaluated as the disc
// out to its band's outer edge, so its band is the disc's rim, its halo lies just inside
// the band and its cast timer fills the inside; only its wash (x) is the band alone.
vec4 decal_coverage(uint index, vec3 world, float aa) {
    float sd;
    float along;
    float rim_width;
    float shape;
    if (!decal_shape(index, world, sd, along, rim_width, shape)) {
        return vec4(0.0);
    }
    float fill = 1.0 - smoothstep(-aa, aa, sd);
    if (fill <= 0.0) {
        return vec4(0.0);
    }
    float rim = fill * (1.0 - smoothstep(rim_width - aa, rim_width + aa, -sd));
    float halo = fill * (1.0 - rim)
        * (1.0 - smoothstep(2.0 * rim_width - aa, 2.0 * rim_width + aa, -sd));
    float progress = 0.0;
    if ((uint(MOBA_DECAL(index).params.y) & MOBA_DECAL_PROGRESS) != 0u) {
        float sweep = MOBA_DECAL(index).params.z;
        progress = fill * (1.0 - smoothstep(sweep - 0.01, sweep + 0.01, along));
    }
    // Rings wash only their band; every other shape washes its whole area.
    float wash = (shape > 0.5 && shape < 1.5) ? rim : fill;
    return vec4(wash, rim, progress, halo);
}

vec3 apply_paint_decals(vec3 albedo, vec3 world, float aa) {
#ifdef MOBA_NO_DECALS
    // Decals off (`terrain_nodecals.frag`): no decal code at all.
    return albedo;
#endif
    int cell = decal_cell(world);
    if (cell < 0) {
        return albedo;
    }
    uvec4 mask = decal_buffer.cells[cell];
    for (int word = 3; word >= 0; word--) {
        uint bits = mask[word];
        while (bits != 0u) {
            int bit = findMSB(bits);
            bits &= ~(1u << uint(bit));
            uint index = uint(word * 32 + bit);
            if ((uint(MOBA_DECAL(index).params.y) & MOBA_DECAL_GLOW) != 0u) {
                continue;
            }
            vec4 coverage = decal_coverage(index, world, aa);
            float alpha = MOBA_DECAL(index).color.a * MOBA_DECAL(index).params.w * coverage.x;
            // A slightly lighter edge, so paint reads as soaked in rather than stamped.
            alpha *= mix(1.0, 0.7, coverage.y);
            albedo = mix(albedo, MOBA_DECAL(index).color.rgb, clamp(alpha, 0.0, 1.0));
        }
    }
    return albedo;
}

vec3 apply_glow_decals(vec3 color, vec3 world, float aa, inout float mark) {
#if defined(MOBA_NO_DECALS) || defined(MOBA_NO_GLOW_DECALS)
    // `MOBA_NO_GLOW_DECALS`: the tiered scene shaders, whose glow decals the post-upscale
    // overlay draws instead (ADR-0021 §5b).
    return color;
#endif
    int cell = decal_cell(world);
    if (cell < 0) {
        return color;
    }
    uvec4 mask = decal_buffer.cells[cell];
    for (int word = 3; word >= 0; word--) {
        uint bits = mask[word];
        while (bits != 0u) {
            int bit = findMSB(bits);
            bits &= ~(1u << uint(bit));
            uint index = uint(word * 32 + bit);
            uint flags = uint(MOBA_DECAL(index).params.y);
            if ((flags & MOBA_DECAL_GLOW) == 0u) {
                continue;
            }
            vec4 coverage = decal_coverage(index, world, aa);
            if (coverage.x + coverage.z + coverage.w <= 0.0) {
                continue;
            }
            float alpha = clamp(MOBA_DECAL(index).color.a * MOBA_DECAL(index).params.w, 0.0, 1.0);
            if ((flags & MOBA_DECAL_MARK) != 0u && alpha > 0.0) {
                mark = max(mark, max(coverage.y, coverage.w));
            }
            float band = min(2.0 * alpha, 1.0);
            vec3 tint = MOBA_DECAL(index).color.rgb;
            // Fill: a light wash, stronger where a cast timer has filled.
            color = mix(color, tint, alpha * (0.3 * coverage.x + 0.35 * coverage.z));
            vec3 rim = tint;
            if ((flags & MOBA_DECAL_ENEMY) != 0u) {
                float ground = decal_luma(color);
                rim *= max(1.0, (1.8 * ground + 0.05) / max(decal_luma(tint), 1e-3));
                color = mix(color, color * 0.2, band * coverage.w);
            }
            color = mix(color, rim, band * coverage.y);
        }
    }
    return color;
}

// The water's variant (R3b): only the glow decals of one role (`enemy` telegraphs or every
// other), so the water can composite enemy telegraphs over everything it shows (the
// transmitted bed included) and the rest on its surface. `rim_cover` returns the strongest
// enemy rim band drawn, so the blended simple water can make its rims opaque. A separate
// function, so the terrain's `apply_glow_decals` is untouched (register budget, §1).
vec3 apply_glow_decals_role(vec3 color, vec3 world, float aa, bool enemy, inout float rim_cover,
                            inout float mark) {
#ifdef MOBA_NO_GLOW_DECALS
    return color;
#endif
    int cell = decal_cell(world);
    if (cell < 0) {
        return color;
    }
    uvec4 mask = decal_buffer.cells[cell];
    for (int word = 3; word >= 0; word--) {
        uint bits = mask[word];
        while (bits != 0u) {
            int bit = findMSB(bits);
            bits &= ~(1u << uint(bit));
            uint index = uint(word * 32 + bit);
            uint flags = uint(MOBA_DECAL(index).params.y);
            if ((flags & MOBA_DECAL_GLOW) == 0u || (((flags & MOBA_DECAL_ENEMY) != 0u) != enemy)) {
                continue;
            }
            vec4 coverage = decal_coverage(index, world, aa);
            if (coverage.x + coverage.z + coverage.w <= 0.0) {
                continue;
            }
            float alpha = clamp(MOBA_DECAL(index).color.a * MOBA_DECAL(index).params.w, 0.0, 1.0);
            if ((flags & MOBA_DECAL_MARK) != 0u && alpha > 0.0) {
                mark = max(mark, max(coverage.y, coverage.w));
            }
            float band = min(2.0 * alpha, 1.0);
            vec3 tint = MOBA_DECAL(index).color.rgb;
            color = mix(color, tint, alpha * (0.3 * coverage.x + 0.35 * coverage.z));
            vec3 rim = tint;
            if (enemy) {
                float ground = decal_luma(color);
                rim *= max(1.0, (1.8 * ground + 0.05) / max(decal_luma(tint), 1e-3));
                color = mix(color, color * 0.2, band * coverage.w);
                rim_cover = max(rim_cover, band * max(coverage.y, coverage.w));
            }
            color = mix(color, rim, band * coverage.y);
        }
    }
    return color;
}

#ifdef MOBA_DECALS_DISPLAY
// ADR-0021 §5b: the overlay's glow decals, composited in display-referred linear space over
// the displayed image (`color` in 0..1) after the tonemap, so telegraphs take no bloom, grade
// or screen effect. The rim rule is the scene's (at least 1.8x the background's luminance plus
// 0.05, a dark halo at 20 %), evaluated against what is displayed; a fixed soft glow outside
// enemy rims replaces the bloom they lose (Q4). `touched` is set where any decal drew.
// `glow_display_reference` in crates/render/src/environment.rs is the CPU twin.
#define MOBA_DECAL_GLOW_REACH 1.5
#define MOBA_DECAL_GLOW_STRENGTH 0.35

vec3 apply_display_glow_decals(vec3 color, vec3 world, float aa, inout bool touched) {
    int cell = decal_cell(world);
    if (cell < 0) {
        return color;
    }
    uvec4 mask = decal_buffer.cells[cell];
    for (int word = 3; word >= 0; word--) {
        uint bits = mask[word];
        while (bits != 0u) {
            int bit = findMSB(bits);
            bits &= ~(1u << uint(bit));
            uint index = uint(word * 32 + bit);
            uint flags = uint(MOBA_DECAL(index).params.y);
            if ((flags & MOBA_DECAL_GLOW) == 0u) {
                continue;
            }
            float alpha = clamp(MOBA_DECAL(index).color.a * MOBA_DECAL(index).params.w, 0.0, 1.0);
            if (alpha <= 0.0) {
                continue;
            }
            float sd;
            float along;
            float rim_width;
            float shape;
            if (!decal_shape(index, world, sd, along, rim_width, shape)) {
                continue;
            }
            float band = min(2.0 * alpha, 1.0);
            vec3 tint = clamp(MOBA_DECAL(index).color.rgb, 0.0, 1.0);
            bool enemy = (flags & MOBA_DECAL_ENEMY) != 0u;
            // The fixed soft glow just outside an enemy rim.
            float reach = MOBA_DECAL_GLOW_REACH * rim_width;
            if (enemy && sd > -aa && sd < reach + aa) {
                float glow = (1.0 - smoothstep(0.0, reach, max(sd, 0.0))) * smoothstep(-aa, aa, sd);
                if (glow > 0.0) {
                    touched = true;
                    color = mix(color, tint, MOBA_DECAL_GLOW_STRENGTH * band * glow);
                }
            }
            vec4 coverage = decal_coverage(index, world, aa);
            if (coverage.x + coverage.z + coverage.w <= 0.0) {
                continue;
            }
            touched = true;
            color = mix(color, tint, alpha * (0.3 * coverage.x + 0.35 * coverage.z));
            vec3 rim = tint;
            if (enemy) {
                float ground = decal_luma(color);
                rim = clamp(rim * max(1.0, (1.8 * ground + 0.05) / max(decal_luma(tint), 1e-3)),
                            0.0, 1.0);
                color = mix(color, color * 0.2, band * coverage.w);
            }
            color = mix(color, rim, band * coverage.y);
        }
    }
    return color;
}
#endif

#endif
