#version 330
#extension GL_ARB_separate_shader_objects : require

// c-LoD pass 2: reduce the 256x256 sun-candidate field to 16x16 tiles.
// Each output texel = (sum w, sum w*u, sum w*v, 1) over its 16x16 tile.

uniform sampler2D ScanSampler; // clod:clod_scan (256x256)

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

void main() {
    ivec2 tile = ivec2(gl_FragCoord.xy);
    ivec2 base = tile * 16;

    float sw = 0.0;
    float su = 0.0;
    float sv = 0.0;
    for (int y = 0; y < 16; ++y) {
        for (int x = 0; x < 16; ++x) {
            vec4 s = texelFetch(ScanSampler, base + ivec2(x, y), 0);
            sw += s.r;
            su += s.r * s.g;
            sv += s.r * s.b;
        }
    }
    fragColor = vec4(sw, su, sv, 1.0);
}
