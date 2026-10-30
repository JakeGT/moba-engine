#version 450

// Fog-of-war resolve (ADR-0014): blurs this frame's raw visibility with a 3x3 tent (four
// bilinear taps at half-texel offsets) and eases the previous frame's result towards it:
// quickly where an area is revealed; where vision is lost, at once down to at most
// `LOST_VISIBILITY` (so a lost area never keeps looking seen), then fading to fog. Written to
// one of two persistent images (ping-pong by frame); the other is `history`. The CPU
// reference is `passes::fog::resolve_reference` (unit-tested).

layout(set = 0, binding = 0) uniform sampler2D raw_visibility;
layout(set = 0, binding = 1) uniform sampler2D history;

layout(push_constant) uniform ResolvePush {
    vec2 texel;    // 1 / resolution
    float reveal;  // blend factor this frame where visibility rises
    float fade;    // blend factor this frame where it falls
} push;

layout(location = 0) out float out_visibility;

void main() {
    vec2 uv = gl_FragCoord.xy * push.texel;
    vec2 h = push.texel * 0.5;
    float blurred = 0.25 * (
        textureLod(raw_visibility, uv + vec2(-h.x, -h.y), 0.0).r
        + textureLod(raw_visibility, uv + vec2(h.x, -h.y), 0.0).r
        + textureLod(raw_visibility, uv + vec2(-h.x, h.y), 0.0).r
        + textureLod(raw_visibility, uv + vec2(h.x, h.y), 0.0).r);
    float previous = textureLod(history, uv, 0.0).r;
    float rate = blurred > previous ? push.reveal : push.fade;
    float gap = blurred - previous;
    // Move at least one R8 step towards the target, so a slow fade at a high frame rate
    // never stalls on the 8-bit quantisation and leaves a faint ghost of old vision.
    const float STEP = 1.01 / 255.0;
    const float LOST_VISIBILITY = 0.5;
    float delta = abs(gap) > STEP ? sign(gap) * max(abs(gap) * rate, STEP) : gap;
    float eased = previous + delta;
    if (gap < 0.0) {
        // Losing vision: never above the current vision or half visible, whichever is more.
        eased = min(eased, max(blurred, LOST_VISIBILITY));
    }
    out_visibility = eased;
}
