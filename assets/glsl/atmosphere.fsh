#version 330
#extension GL_ARB_separate_shader_objects : require

// c-LoD atmosphere: a distance veil over real terrain that hides the
// render-distance edge. Pure depth-driven (no view rotation needed), so it
// works on every frame, before the LOD chain ever sees the sun.
//
// Depth thresholds are uniforms rather than reconstructed distances: the
// device->eye mapping differs between renderpearl backends, and a monotonic
// smoothstep over device depth is backend-independent and fully tunable.

uniform sampler2D In;      // minecraft:main color
uniform sampler2D InDepth; // minecraft:main depth

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

// x = veil start (device depth), y = veil end (device depth), z = strength, w = unused
layout(std140) uniform ClodAtmo {
    vec4 AtmoParams;
};

layout(std140) uniform ClodAtmoColor {
    vec4 VeilColor;
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

const float SKY_DEPTH = 0.99995;

void main() {
    vec4 scene = texture(In, texCoord);
    float depth = texture(InDepth, texCoord).r;

    // Sky pixels stay vanilla: the LOD chain paints distant terrain there.
    if (depth < SKY_DEPTH || depth <= 0.0) {
        fragColor = scene;
        return;
    }

    float veil = smoothstep(AtmoParams.x, AtmoParams.y, depth) * AtmoParams.z;
    fragColor = vec4(mix(scene.rgb, VeilColor.rgb, clamp(veil, 0.0, 1.0)), scene.a);
}
