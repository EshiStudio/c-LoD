#version 330
#extension GL_ARB_separate_shader_objects : require

// c-LoD pass 2 (v4.1): decode the camera basis from the channel scan.
//
// Picks the topmost intact payload row across all col_scan slices, decodes
// azimuth / elevation / fov and writes the persistent rotation target in the
// EXACT v4.0 encoding consumed by clod:post/lod_terrain:
//
//   R = az / (2*pi),  G = (el + pi/2) / pi,  B = confidence,  A = fov byte.
//
// The payload is measured, not fitted — no blending, no warm-up: confidence
// jumps straight to 1 on the first valid row and the basis is frame-exact
// (blending would make the LOD horizon swim behind the true one). When the
// column is fully occluded (facing a wall of terrain), the previous estimate
// decays at 0.99/frame (~3 s) and the LOD fades instead of popping — same
// policy as the v4.0 sun tracker, minus the sun dependency entirely.

uniform sampler2D ScanSampler; // clod:clod_scan (640x1 slice report)
uniform sampler2D InSampler;   // minecraft:main (payload rows)
uniform sampler2D PrevSampler; // clod:clod_rot_a (persistent)

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

const int SLICES = 640;
const float PI = 3.14159265358979;

void main() {
    vec4 prev = texelFetch(PrevSampler, ivec2(0), 0);
    vec4 rot = vec4(prev.r, prev.g, prev.b * 0.99, prev.a);

    int best = -1;
    for (int j = 0; j < SLICES; ++j) {
        vec4 s = texelFetch(ScanSampler, ivec2(j, 0), 0);
        if (s.r > 0.5) {
            int y = int(s.g * 255.0 + 0.5) * 256 + int(s.b * 255.0 + 0.5);
            if (y > best) {
                best = y;
            }
        }
    }

    if (best >= 0) {
        float a[6];
        for (int i = 2; i < 8; ++i) {
            a[i - 2] = texelFetch(InSampler, ivec2(i, best), 0).a * 255.0;
        }
        float azq = a[0] * 256.0 + a[1];
        float elq = a[2] * 256.0 + a[3];
        float fovq = a[4];

        float az = azq / 65535.0 * 6.28318530718;
        float el = elq / 65535.0 * PI - PI * 0.5;
        rot = vec4(az / 6.28318530718, (el + PI * 0.5) / PI, 1.0, fovq / 254.0);
    }

    fragColor = rot;
}
