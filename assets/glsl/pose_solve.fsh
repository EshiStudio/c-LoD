#version 330
#extension GL_ARB_separate_shader_objects : require
#include <minecraft:globals.glsl>

// c-LoD pass 4: camera pose from the depth buffer.
//
// The sun/moon tracker only works while the disc is on screen. At noon the sun
// sits at the zenith, so a player looking at the horizon NEVER gets a lock and
// the LOD pass would freeze on a stale (or empty) pose — no distant terrain at
// all. The real chunk terrain, however, is always in view, and it is exactly
// what the atlas contains, so the pose can be recovered from the scene:
//
//   device depth d  ->  eyeZ = depth_near / d
//   world point      P = cam + dir(az, el, ndc) * eyeZ * |v|
//   atlas constraint   atlas_height(P.xz) == P.y
//
// so (az, el) is the rotation that makes the residuals vanish. With no usable
// previous pose the pass runs a coarse global search (24 x 9), then three
// Gauss-Newton steps per frame (robust/Huber weighted, never worse than the
// previous frame's pose).

uniform sampler2D PrevSampler;      // clod_rot_b (1x1: az, el, conf, 1)
uniform sampler2D InDepthSampler;   // minecraft:main depth buffer
uniform sampler2D AtlasSampler;     // clod:atlas (RG = height, B = water)

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

// x = fov (deg), y = depth near (blocks), z = max height, w = sea level
layout(std140) uniform ClodPose {
    vec4 PoseParams;
};

// xy = atlas origin (min corner, blocks), z = atlas size (blocks), w = cell
layout(std140) uniform ClodAtlas {
    vec4 AtlasInfo;
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

const float PI = 3.14159265358979;
const int GRID = 7;
const float SAMPLE_LO = -0.92;
const float SAMPLE_HI = 0.92;

vec2 grid_ndc(int ix, int iy) {
    return vec2(
        mix(SAMPLE_LO, SAMPLE_HI, float(ix) / float(GRID - 1)),
        mix(SAMPLE_LO, SAMPLE_HI, float(iy) / float(GRID - 1))
    );
}

float atlas_h(vec2 xz) {
    vec2 uv = clamp(
        (xz - AtlasInfo.xy) / max(AtlasInfo.z, 1.0),
        vec2(0.0015),
        vec2(0.9985)
    );
    vec4 a = texture(AtlasSampler, uv);
    float h = (a.r * 65280.0 + a.g * 255.0) / 64.0 - 64.0;
    return a.b > 0.5 ? PoseParams.w : h;
}

vec3 cam_pos() {
    // 26.3 Globals: CameraBlockPos = floor(camPos), CameraOffset = floor - camPos.
    return vec3(CameraBlockPos) - CameraOffset;
}

vec3 dir_ndc(vec2 ndc, float az, float el, float tanH, float aspect) {
    float ce = cos(el);
    float se = sin(el);
    float ca = cos(az);
    float sa = sin(az);
    vec3 F = vec3(ce * ca, se, ce * sa);
    vec3 R = vec3(-sa, 0.0, ca);
    vec3 U = vec3(-ca * se, ce, -sa * se);
    return normalize(F + R * (ndc.x * tanH * aspect) + U * (ndc.y * tanH));
}

// Residual in blocks for one screen sample; w = 0 when the pixel is sky.
float resid(vec2 ndc, float az, float el, float tanH, float aspect, out float w) {
    w = 0.0;
    float d = texture(InDepthSampler, ndc * 0.5 + 0.5).r;
    if (d <= 1.0e-6) {
        return 0.0;
    }
    float eyeZ = PoseParams.y / max(d, 1.0e-9);
    float len = max(length(vec3(ndc.x * tanH * aspect, ndc.y * tanH, 1.0)), 1.0);
    vec3 P = cam_pos() + dir_ndc(ndc, az, el, tanH, aspect) * eyeZ * len;
    w = 1.0;
    // Robust: trees, player builds and 4-block atlas cells legitimately differ.
    // Wide clamp: the gradient must survive a badly wrong starting pose.
    return clamp(atlas_h(P.xz) - P.y, -128.0, 128.0);
}

// Robust mean absolute residual over a subsampled GRID x GRID lattice.
// L1 (not squared, not 8-block capped) is essential: with the old cap every
// wrong pose cost exactly the same, so the coarse search had a flat surface and
// Gauss-Newton saw zero gradient -- the tracker could never recover from a bad
// seed and `conf` reported 1.0 no matter what.
float cost(float az, float el, float tanH, float aspect, int stride, out float nOut) {
    float sum = 0.0;
    float n = 0.0;
    for (int iy = 0; iy < GRID; ++iy) {
        if (stride > 1 && (iy % stride) != 0) {
            continue;
        }
        for (int ix = 0; ix < GRID; ++ix) {
            if (stride > 1 && (ix % stride) != 0) {
                continue;
            }
            float w;
            float r = resid(grid_ndc(ix, iy), az, el, tanH, aspect, w);
            sum += w * min(abs(r), 256.0);
            n += w;
        }
    }
    nOut = n;
    return n >= 4.0 ? sum / n : 1.0e6;
}

void main() {
    vec4 prev = texelFetch(PrevSampler, ivec2(0), 0);
    float prevAz = prev.r * 6.28318530718;
    float prevEl = prev.g * PI - PI * 0.5;
    float prevConf = prev.b;

    // FOV is estimated too (channel a of the pose target): the vanilla slider
    // is 30..110 deg and options.txt stores a normalised value, so the pack
    // config is only a seed. Dynamic FOV (sprint/fly) is tracked as well.
    float fovDeg = prev.a > 0.02 ? 30.0 + prev.a * 80.0 : PoseParams.x;
    float tanH = tan(radians(fovDeg) * 0.5);
    // This pass renders into a 1x1 target, so its OutSize/InSize are useless:
    // the screen aspect comes from the Globals UBO (same value the LOD pass
    // sees as OutSize, since that pass writes the full framebuffer).
    vec2 scr = ScreenSize.x > 1.0 ? ScreenSize : vec2(max(InSize.x, 1.0), max(InSize.y, 1.0));
    float aspect = max(scr.x, 1.0) / max(scr.y, 1.0);

    float az = prevAz;
    float el = prevEl;
    bool boot = prevConf < 0.05;

    // Coarse global search: 48 x 25 (7.5 deg). Runs on a 1x1 target, so the
    // ~20k atlas fetches below cost nothing measurable.
    if (boot) {
        float best = 1.0e18;
        for (int a = 0; a < 48; ++a) {
            float taz = float(a) * (2.0 * PI / 48.0);
            for (int e = 0; e < 25; ++e) {
                float tel = radians(-90.0 + 7.5 * float(e));
                float cn;
                float c = cost(taz, tel, tanH, aspect, 2, cn);
                if (c < best) {
                    best = c;
                    az = taz;
                    el = tel;
                }
            }
        }
        if (best > 1.0e5) {
            // No terrain in view at all: keep whatever the sun tracker had.
            fragColor = prev;
            return;
        }
    }

    // Gauss-Newton on (az, el, tanH): robust weighted, Levenberg damped.
    for (int it = 0; it < 5; ++it) {
        float a00 = 0.0;
        float a01 = 0.0;
        float a02 = 0.0;
        float a11 = 0.0;
        float a12 = 0.0;
        float a22 = 0.0;
        float b0 = 0.0;
        float b1 = 0.0;
        float b2 = 0.0;
        float n = 0.0;
        const float d = 0.01;
        const float dt = 0.004;
        for (int iy = 0; iy < GRID; ++iy) {
            for (int ix = 0; ix < GRID; ++ix) {
                vec2 ndc = grid_ndc(ix, iy);
                float w0;
                float w1;
                float w2;
                float w3;
                float r0 = resid(ndc, az, el, tanH, aspect, w0);
                if (w0 < 0.5) {
                    continue;
                }
                float ra = resid(ndc, az + d, el, tanH, aspect, w1);
                float rb = resid(ndc, az, el + d, tanH, aspect, w2);
                float rt = resid(ndc, az, el, tanH + dt, aspect, w3);
                float ja = (ra - r0) / d;
                float jb = (rb - r0) / d;
                float jt = (rt - r0) / dt;
                float hw = 1.0 / (1.0 + abs(r0) / 24.0);
                a00 += hw * ja * ja;
                a01 += hw * ja * jb;
                a02 += hw * ja * jt;
                a11 += hw * jb * jb;
                a12 += hw * jb * jt;
                a22 += hw * jt * jt;
                b0 += hw * ja * r0;
                b1 += hw * jb * r0;
                b2 += hw * jt * r0;
                n += 1.0;
            }
        }
        if (n < 6.0) {
            break;
        }
        // Damping keeps the near-degenerate FOV direction from exploding.
        float lam = 1.0e-3 * (a00 + a11 + a22) / 3.0 + 1.0e-6;
        a00 += lam;
        a11 += lam;
        a22 += lam;

        float c00 = a11 * a22 - a12 * a12;
        float c01 = a02 * a12 - a01 * a22;
        float c02 = a01 * a12 - a02 * a11;
        float c11 = a00 * a22 - a02 * a02;
        float c12 = a01 * a02 - a00 * a12;
        float c22 = a00 * a11 - a01 * a01;
        float det = a00 * c00 + a01 * c01 + a02 * c02;
        if (abs(det) < 1.0e-9) {
            break;
        }
        float dz = -(c00 * b0 + c01 * b1 + c02 * b2) / det;
        float de = -(c01 * b0 + c11 * b1 + c12 * b2) / det;
        float dtan = -(c02 * b0 + c12 * b1 + c22 * b2) / det;
        az += clamp(dz, -0.35, 0.35);
        el = clamp(el + clamp(de, -0.35, 0.35), -1.55, 1.55);
        tanH = clamp(tanH + clamp(dtan, -0.06, 0.06), 0.25, 1.45);
    }

    float nUse;
    float rms = cost(az, el, tanH, aspect, 1, nUse);
    if (nUse < 3.0) {
        // Nothing to fit (sky only): leave the previous estimate untouched.
        fragColor = prev;
        return;
    }

    // Never accept a pose that explains the terrain worse than the previous one.
    if (!boot) {
        float nPrev;
        float rmsPrev = cost(prevAz, prevEl, tanH, aspect, 1, nPrev);
        if (rms > rmsPrev) {
            az = prevAz;
            el = prevEl;
            rms = rmsPrev;
        }
    }

    // Mean |height error| in blocks: 4-block atlas cells + trees sit around 2-5.
    float conf = rms < 7.0 ? 1.0 : max(prevConf * 0.85, 0.0);
    az = mod(az, 2.0 * PI);
    float fovOut = degrees(2.0 * atan(tanH));
    float fovCh = clamp((fovOut - 30.0) / 80.0, 0.0, 1.0);
    fragColor = vec4(az / (2.0 * PI), (el + PI * 0.5) / PI, conf, fovCh);
}
