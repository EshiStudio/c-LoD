#version 330
#extension GL_ARB_separate_shader_objects : require

// c-LoD pass 4: ping-pong copy for the persistent rotation target
// (clod_rot_b -> clod_rot_a) so the next frame seeds from the newest solve
// without ever reading and writing the same target in one pass.

uniform sampler2D InSampler;

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

void main() {
    fragColor = texelFetch(InSampler, ivec2(0, 0), 0);
}
