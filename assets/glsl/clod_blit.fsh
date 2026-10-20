#version 330
#extension GL_ARB_separate_shader_objects : require

// c-LoD pass 6: composite the LOD frame back into minecraft:main.

uniform sampler2D InSampler;

layout(std140) uniform BlitConfig {
    vec4 ColorModulate;
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

void main() {
    fragColor = texture(InSampler, texCoord) * ColorModulate;
}
