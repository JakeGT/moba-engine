#version 450
#extension GL_GOOGLE_include_directive : require

// Sky backdrop (ADR-0014): the map's environment cube at a blurred level when the view samples
// it (Medium and up, with an environment), else a zenith/horizon gradient with a sun glow.
// Towards and below the horizon it blends into the height-fog colour, so distant terrain
// fades into the sky without a seam.

#include "camera.glsl"

layout(set = 3, binding = 2) uniform samplerCube environment_specular;

layout(location = 0) in vec3 in_ray;

layout(location = 0) out vec4 out_color;

void main() {
    vec3 dir = normalize(in_ray);
    float up = dir.y;
    vec3 color;
    if (camera.sky_zenith.w > 0.5) {
        color = textureLod(environment_specular, dir, camera.sky_horizon.w).rgb;
    } else {
        float height = pow(clamp(up, 0.0, 1.0), 0.55);
        color = mix(camera.sky_horizon.rgb, camera.sky_zenith.rgb, height);
        // A soft glow around the sun (the sun itself is the shaded key light; no disk).
        float sun = max(dot(dir, normalize(camera.light_direction.xyz)), 0.0);
        color += camera.sun_color.rgb * camera.light_direction.w
            * (pow(sun, 48.0) * 0.35 + pow(sun, 6.0) * 0.05);
    }
    // Horizon haze: the fog colour near and below the horizon.
    vec3 horizon = camera.haze_color.w > 0.0 ? camera.haze_color.rgb : camera.sky_horizon.rgb;
    float haze = 1.0 - smoothstep(-0.02, 0.22, up);
    color = mix(color, horizon, haze);
    out_color = vec4(min(color, vec3(32768.0)), 1.0);
}
