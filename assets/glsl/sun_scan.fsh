#version 330
#extension GL_ARB_separate_shader_objects : require
#include <minecraft:globals.glsl>

// c-LoD pass 1: sun candidate scan.
// For every screen pixel that shows sky (device depth ~ 1.0), estimate how
// likely it belongs to the sun (day) or moon (night) disc. The vanilla sun
// travels in the world X-Y plane: dir = (cos t, sin t, 0), t = 2*pi*fract(GameTime).
// The result is a weight field reduced by the next passes into a 1x1
// rotation estimate — this is how c-LoD recovers the view direction without
// any client mod: everything comes from the scene itself.

uniform sampler2D InSampler;      // minecraft:main (color)
uniform sampler2D InDepthSampler; // minecraft:main (depth buffer)

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

// x = fov (deg), y = warmth threshold, z = min luma, w = unused
layout(std140) uniform ClodScan {
    vec4 ScanParams;
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

const float GEOMETRY_DEPTH = 1.0e-6;

void main() {
    float depth = texture(InDepthSampler, texCoord).r;
    vec3 col = texture(InSampler, texCoord).rgb;

    // 26.3 device depth (live-verified): d = near/eyeZ, infinite far plane —
    // the sky sits at EXACTLY 0.0; any real geometry reads positive values.
    // Sky pixels only: real geometry (and the LOD we draw later) never
    // writes to the far plane.
    if (depth > GEOMETRY_DEPTH) {
        fragColor = vec4(0.0);
        return;
    }

    // Day fraction: GameTime is ticks/24000 (+ partial tick), fract() handles
    // both absolute-tick and day-fraction interpretations.
    float t = fract(GameTime) * 6.28318530718;
    vec3 sunDir = vec3(cos(t), sin(t), 0.0);

    float luma = dot(col, vec3(0.2126, 0.7152, 0.0722));

    // Below horizon or heavily overcast: nothing to track this frame.
    float weight = 0.0;
    if (sunDir.y >= -0.12) {
        // DAY: the sun quad is warm (red > blue). Clouds are neutral and
        // washed out — the warmth^2 * luma^2 kernel suppresses them.
        float warmth = max(0.0, col.r - max(col.g, col.b) * 0.92);
        warmth = max(warmth, 0.004) * step(0.004, warmth);
        weight = luma * luma * warmth * warmth * 4096.0;
    } else {
        // NIGHT: track the moon (opposite the sun, neutral white, dominant).
        weight = max(0.0, luma - 0.55) * max(0.0, luma - 0.55) * 256.0
                 * step(col.b + 0.02, col.r + 0.08);
    }

    // Confidence gate: a lone bright pixel (star, glint) must not steer the
    // tracker; the reduce pass thresholds on the total weight anyway.
    if (weight < ScanParams.y) {
        weight = 0.0;
    }

    fragColor = vec4(weight, gl_FragCoord.x / OutSize.x, gl_FragCoord.y / OutSize.y, 1.0);
}
