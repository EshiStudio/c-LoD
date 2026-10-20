#version 330
#extension GL_ARB_separate_shader_objects : require

// c-LoD pass 3: solve the view rotation from the tracked sun (or moon) blob.
//
// Camera basis from (az, el):
//   F = (cos e * cos az, sin e, cos e * sin az)   forward
//   R = (-sin az, 0, cos az)                      right
//   U = cross(R, F) = (-cos az * sin e, cos e, -sin az * sin e)
//
// The sun's world direction w is known from GameTime. Its predicted screen
// position is p(az, e). We fit (az, e) to the measured blob centroid with a
// tiny Gauss-Newton solver (finite differences), seeded by the previous
// frame — then blend into a persistent 1x1 rotation target:
//   R = az / (2*pi), G = (e + pi/2) / pi, B = confidence.
//
// If nothing was detected, the previous estimate decays instead of being
// corrupted. LOD rendering keys off confidence.

uniform sampler2D SmallSampler; // clod:clod_scan_small (16x16 tile sums)
uniform sampler2D PrevSampler;  // clod:clod_rot_a  (1x1 persistent)

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

// x = fov (deg), y = rot blend, z = total-weight threshold, w = unused
layout(std140) uniform ClodSolve {
    vec4 SolveParams;
};

#include <minecraft:globals.glsl>

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

const float PI = 3.14159265358979;

vec3 sun_world_dir() {
    float t = fract(GameTime) * 6.28318530718;
    vec3 sun = vec3(cos(t), sin(t), 0.0);
    if (sun.y < -0.12) {
        sun = -sun; // night: track the moon
    }
    return sun;
}

// Predicted normalized screen position of the sun for a (az, el) camera.
vec2 predict(vec3 w, float az, float el, float tanH, float aspect) {
    vec3 F = vec3(cos(el) * cos(az), sin(el), cos(el) * sin(az));
    vec3 R = vec3(-sin(az), 0.0, cos(az));
    vec3 U = vec3(-cos(az) * sin(el), cos(el), -sin(az) * sin(el));
    vec3 c = vec3(dot(w, R), dot(w, U), dot(w, F));
    if (c.z <= 0.0) {
        return vec2(-1.0e9); // behind the camera: huge residual
    }
    float sx = c.x / c.z;
    float sy = c.y / c.z;
    return vec2(sx / (2.0 * tanH * aspect) + 0.5, sy / (2.0 * tanH) + 0.5);
}

float wrap_pi(float a) {
    return a - 6.28318530718 * floor((a + PI) / 6.28318530718);
}

void main() {
    float fov = SolveParams.x;
    float blend = SolveParams.y;
    float wThresh = SolveParams.z;

    float tanH = tan(radians(fov) * 0.5);
    float aspect = OutSize.x / max(OutSize.y, 1.0);

    // Find the strongest tile, then use a 3x3 neighborhood around it so the
    // centroid covers the whole disc, not a clipped tile.
    float best = 0.0;
    ivec2 bestTile = ivec2(0);
    for (int y = 0; y < 16; ++y) {
        for (int x = 0; x < 16; ++x) {
            float w = texelFetch(SmallSampler, ivec2(x, y), 0).r;
            if (w > best) {
                best = w;
                bestTile = ivec2(x, y);
            }
        }
    }

    float sw = 0.0;
    float su = 0.0;
    float sv = 0.0;
    for (int y = -1; y <= 1; ++y) {
        for (int x = -1; x <= 1; ++x) {
            ivec2 t = bestTile + ivec2(x, y);
            if (any(lessThan(t, ivec2(0))) || any(greaterThanEqual(t, ivec2(16)))) {
                continue;
            }
            vec4 s = texelFetch(SmallSampler, t, 0);
            sw += s.r;
            su += s.r * s.g;
            sv += s.r * s.b;
        }
    }

    vec4 prev = texelFetch(PrevSampler, ivec2(0), 0);
    float prevAz = prev.r * 6.28318530718;
    float prevEl = prev.g * PI - PI * 0.5;
    float prevConf = prev.b;

    vec4 outRot = vec4(prev.r, prev.g, prevConf * 0.94, 1.0);

    if (sw >= wThresh) {
        vec2 meas = vec2(su / sw, sv / sw);
        vec3 w = sun_world_dir();

        // Seed: previous frame (or face the sun horizontally).
        float az = prevConf > 0.05 ? prevAz : atan(w.z, w.x);
        float el = prevConf > 0.05 ? prevEl : 0.0;

        // Gauss-Newton, 2 iterations, forward differences.
        for (int it = 0; it < 2; ++it) {
            vec2 r0 = predict(w, az, el, tanH, aspect) - meas;
            float d = 0.0005;
            vec2 rx = predict(w, az + d, el, tanH, aspect) - meas;
            vec2 ry = predict(w, az, el + d, tanH, aspect) - meas;
            vec2 j0 = (rx - r0) / d;
            vec2 j1 = (ry - r0) / d;

            float a00 = dot(j0, j0) + 1.0e-6;
            float a01 = dot(j0, j1);
            float a11 = dot(j1, j1) + 1.0e-6;
            float b0 = -dot(j0, r0);
            float b1 = -dot(j1, r0);

            float det = a00 * a11 - a01 * a01;
            if (abs(det) > 1.0e-9) {
                float dAz = (a11 * b0 - a01 * b1) / det;
                float dEl = (a00 * b1 - a01 * b0) / det;
                az += clamp(dAz, -0.2, 0.2);
                el += clamp(dEl, -0.2, 0.2);
            }
        }
        el = clamp(el, -PI * 0.495, PI * 0.495);
        az = wrap_pi(az);

        // Temporal blend (azimuth via shortest arc).
        float mixedAz = prevAz + wrap_pi(az - prevAz) * blend;
        float mixedEl = mix(prevEl, el, blend);
        float conf = min(1.0, max(prevConf, 0.4) + 0.1);

        outRot = vec4(mixedAz / 6.28318530718, (mixedEl + PI * 0.5) / PI, conf, 1.0);
    }

    fragColor = outRot;
}
