#version 450

// Screen-space team/selection outlines (High and up, ADR-0016), after the tonemap so the
// palette colour reaches the screen exactly. Each pixel looks for a higher-priority slot
// within its width in eight directions in the ID target the depth pre-pass wrote, and
// blends that slot's colour with coverage anti-aliasing on the outermost pixel. Only the
// exterior of a silhouette (a lower slot, or no unit) is drawn.

layout(set = 0, binding = 0) uniform usampler2D outline_ids;

layout(push_constant) uniform OutlinePush {
    // Slot 0: x the widest slot (pixels, at most 4), y 1 = the target is *_SRGB, zw the last
    // pixel of this frame's rendered sub-rectangle (render scale): taps never read past it.
    // Slots 1..7: rgb linear display colour, w width in pixels (0 = off).
    vec4 palette[8];
} push;

layout(location = 0) out vec4 out_color;

const vec2 DIRECTIONS[8] = vec2[8](
    vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0),
    vec2(0.7071, 0.7071), vec2(-0.7071, 0.7071), vec2(0.7071, -0.7071), vec2(-0.7071, -0.7071));

void main() {
    ivec2 pixel = ivec2(gl_FragCoord.xy);
    ivec2 last = min(ivec2(push.palette[0].zw), textureSize(outline_ids, 0) - 1);
    uint own = texelFetch(outline_ids, pixel, 0).r;
    // 8 marks a unit without an outline (ADR-0019's unit mask): outlined like no unit.
    if (own >= 8u) {
        own = 0u;
    }
    int reach = min(int(ceil(push.palette[0].x)), 4);
    uint best = 0u;
    float coverage = 0.0;
    for (int distance = 1; distance <= 4; distance++) {
        if (distance > reach) {
            break;
        }
        for (int direction = 0; direction < 8; direction++) {
            ivec2 tap = clamp(pixel + ivec2(round(DIRECTIONS[direction] * float(distance))),
                              ivec2(0), last);
            uint id = texelFetch(outline_ids, tap, 0).r;
            if (id <= own || id >= 8u) {
                continue;
            }
            float cover = clamp(push.palette[id].w - float(distance) + 1.0, 0.0, 1.0);
            if (cover <= 0.0) {
                continue;
            }
            // The highest slot wins; within it, the nearest (fullest) tap.
            if (id > best) {
                best = id;
                coverage = cover;
            } else if (id == best) {
                coverage = max(coverage, cover);
            }
        }
    }
    if (best == 0u) {
        // Zero alpha leaves the pixel as it is (no `discard`: it needs the demote feature
        // under the Vulkan 1.3 target).
        out_color = vec4(0.0);
        return;
    }
    vec3 color = push.palette[best].rgb;
    // A UNORM target stores what we write: encode it (sRGB targets encode on store).
    if (push.palette[0].y < 0.5) {
        color = pow(color, vec3(1.0 / 2.2));
    }
    out_color = vec4(color, coverage);
}
