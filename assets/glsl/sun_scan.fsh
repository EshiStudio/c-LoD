#version 330
#extension GL_ARB_separate_shader_objects : require
#include <minecraft:globals.glsl>

// c-LoD pass 1: sun/moon candidate scan.
//
// Every sky pixel is scored 0..1 for "this looks like the celestial body".
// The score, its screen position and the sky mask are reduced by the next
// passes into a 1x1 rotation estimate — this is how c-LoD recovers the view
// direction without any client mod: everything comes from the scene itself.
//
// The vanilla sprites are strongly coloured:
//   assets/minecraft/textures/environment/celestial/sun.png    -> (1,1,0)
//   assets/minecraft/textures/environment/celestial/moon/*.png -> (0,0,1)
// so "yellow-ness" = min(r,g) - b separates the day sun from white clouds and
// blue sky, and "blue-ness" = b - max(r,g) finds the moon at night.
// (The old r - 0.92*max(g,b) kernel scored white clouds and the yellow sun
// identically — 0.08 — so the tracker could never isolate the disc.)
//
// IMPORTANT: post-chain internal targets are 8-bit (PostChainConfig's
// InternalTarget has no format field), so EVERY value written here must stay
// inside 0..1. The reduce pass averages (never sums) for the same reason.

uniform sampler2D InSampler;      // minecraft:main (color)
uniform sampler2D InDepthSampler; // minecraft:main (depth buffer)

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

// x = fov (deg), y = noise gate (0..1), z,w unused
layout(std140) uniform ClodScan {
    vec4 ScanParams;
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

// 26.3 device depth: d = near/eyeZ (infinite far) — sky == 0.0.
const float SKY_DEPTH = 1.0e-6;

void main() {
    float depth = texture(InDepthSampler, texCoord).r;
    vec3 col = texture(InSampler, texCoord).rgb;

    // Sky pixels only: real geometry never writes to the far plane.
    if (depth > SKY_DEPTH) {
        fragColor = vec4(0.0);
        return;
    }

    // Day fraction: GameTime = (ticks % 24000 + partial) / 24000.
    float t = fract(GameTime) * 6.28318530718;
    vec3 sunDir = vec3(cos(t), sin(t), 0.0);

    float luma = dot(col, vec3(0.2126, 0.7152, 0.0722));
    float w = 0.0;

    if (sunDir.y >= -0.12) {
        // DAY: yellow disc on a blue/white sky.
        float yellow = min(col.r, col.g) - col.b;
        w = clamp(yellow * 1.7, 0.0, 1.0) * smoothstep(0.30, 0.70, luma);
    } else {
        // NIGHT: blue-white moon disc on a dark sky.
        float blue = col.b - max(col.r, col.g);
        w = clamp(blue * 3.0, 0.0, 1.0) * smoothstep(0.22, 0.55, luma);
    }

    // Noise gate (0..1 units: a lone star or a glint must not steer us).
    if (w < ScanParams.y) {
        w = 0.0;
    }

    fragColor = vec4(w, gl_FragCoord.x / OutSize.x, gl_FragCoord.y / OutSize.y, 1.0);
}
