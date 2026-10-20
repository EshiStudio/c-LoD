#version 330
#extension GL_ARB_separate_shader_objects : require
#include <minecraft:globals.glsl>

// c-LoD atmosphere: a distance veil over real terrain that hides the
// render-distance edge. Pure depth-driven (no view rotation needed), so it
// works on every frame, before the LOD chain ever sees the sun.
//
// Depth thresholds are uniforms rather than reconstructed distances: the
// device->eye mapping differs between renderpearl backends, and a monotonic
// smoothstep over device depth is backend-independent and fully tunable.
// The veil color tracks the day cycle through GameTime (post chains receive
// Globals but no Fog UBO — verified against the 26.3 client sources).

uniform sampler2D InSampler;      // minecraft:main color
uniform sampler2D InDepthSampler; // minecraft:main depth

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

// x = veil start (blocks), y = veil end (blocks), z = strength, w = unused
layout(std140) uniform ClodAtmo {
    vec4 AtmoParams;
};

// 26.3 device-depth conversion: d = near/eyeZ (infinite far), so eye-space
// BLOCK distance = DepthNear / max(d, 1e-9). The sky and the sun/moon quads
// sit at EXACTLY 0.0 and must never be veiled — veiling them starves the
// sun-tracker in the LOD chain (live-debugged on a real 26.3 client).
layout(std140) uniform ClodDepth {
    vec4 DepthParams; // x = depth near (blocks), yzw = unused
};

layout(std140) uniform ClodAtmoColor {
    vec4 VeilColor;
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

const float SKY_DEPTH = 1.0e-6;

void main() {
    vec4 scene = texture(InSampler, texCoord);
    float depth = texture(InDepthSampler, texCoord).r;

    // Sky AND celestial bodies stay vanilla: they sit at device depth 0.0
    // (infinite far), while ANY real geometry reads positive depth.
    if (depth < SKY_DEPTH || depth <= 0.0) {
        fragColor = scene;
        return;
    }

    // Device depth -> eye-space blocks, then a plain distance veil.
    float eyeZ = DepthParams.x / max(depth, 1.0e-9);

    // Day-graded veil: bright and neutral by day, darker moonlit blue at
    // night, so the edge never glows against a dark sky.
    float sunY = sin(fract(GameTime) * 6.28318530718);
    float dayF = smoothstep(-0.14, 0.22, sunY);
    vec3 veilCol = VeilColor.rgb * mix(0.42, 1.0, dayF);
    veilCol = mix(veilCol * vec3(0.85, 0.9, 1.12), veilCol, dayF);

    float veil = smoothstep(AtmoParams.x, AtmoParams.y, eyeZ) * AtmoParams.z;
    fragColor = vec4(mix(scene.rgb, veilCol, clamp(veil, 0.0, 1.0)), scene.a);
}
