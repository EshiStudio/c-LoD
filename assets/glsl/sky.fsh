#version 330
#extension GL_ARB_separate_shader_objects : require

#include <minecraft:fog.glsl>
#include <minecraft:dynamictransforms.glsl>
#include <minecraft:projection.glsl>

layout(location = 0) in float sphericalVertexDistance;
layout(location = 1) in float cylindricalVertexDistance;

layout(location = 0) out vec4 fragColor;

// c-LoD camera channel (v4.1) — replaces the v4.0 sun tracker.
//
// The sky program is the only vanilla stage that receives the true camera
// rotation (ModelViewMat: the camera-relative world pass loads the global
// modelview stack with the rotation only) and the projection (ProjMat).
// We encode the view basis into the ALPHA channel of the leftmost 8 columns
// of every sky fragment. RGB stays 100% vanilla — the channel is invisible
// on screen (alpha of the scene target is never displayed).
//
//   x0 = 0xC7, x1 = 0x63          magic (2-byte, false positive ~2^-16/row)
//   x2,x3 = azimuth               big-endian, az/2pi * 65535
//   x4,x5 = elevation             big-endian, (el+pi/2)/pi * 65535
//   x6 = fov byte                 (fovDeg - 5) / 115 * 254  (255 = invalid)
//   x7 = checksum                 (x0..x6 additive) mod 256
//
// Basis convention is IDENTICAL to the tracker this replaces (see
// clod:post/lod_terrain): F = (cos el cos az, sin el, cos el sin az).
//
// Downstream writes (terrain alpha = 1.0, clouds, particles, weather, GUI)
// fail the magic/checksum checks; the post chain scans the column top-down
// for the topmost INTACT row, so transient overdraw self-heals within a
// frame for as long as any sky row is visible in the leftmost 8 columns.
//
// Vertically inverted note: gl_FragCoord (0,0) is the BOTTOM-left texel;
// the "top of the screen" is simply large y — no viewport size is needed.
void main() {
    vec4 color = apply_fog(ColorModulator, sphericalVertexDistance,
        cylindricalVertexDistance, 0.0, FogSkyEnd, FogSkyEnd, FogSkyEnd, FogColor);

    ivec2 px = ivec2(gl_FragCoord.xy);
    if (px.x < 8) {
        // View forward in world space: view space looks down -Z, and
        // transpose(mat3(MV)) is the inverse of the rotation-only modelview.
        vec3 fwd = transpose(mat3(ModelViewMat)) * vec3(0.0, 0.0, -1.0);
        float az = atan(fwd.z, fwd.x);                          // (-pi, pi]
        float el = asin(clamp(fwd.y, -1.0, 1.0));               // [-pi/2, pi/2]
        // Vertical FOV from the projection: ProjMat[1][1] = 1 / tan(fovY/2).
        // The 26.3 infinite-far depth remap only touches the z rows.
        float fovDeg = degrees(atan(1.0 / ProjMat[1][1])) * 2.0;

        float a16 = floor(fract(az / 6.28318530718) * 65535.0 + 0.5);
        float e16 = floor(clamp((el + 1.57079632679) / 3.14159265359, 0.0, 1.0) * 65535.0 + 0.5);
        int azh = int(floor(a16 / 256.0));
        int azl = int(mod(a16, 256.0));
        int elh = int(floor(e16 / 256.0));
        int ell = int(mod(e16, 256.0));
        int fovq = int(clamp((fovDeg - 5.0) / 115.0, 0.0, 1.0) * 254.0 + 0.5);
        int chk = (199 + 99 + azh + azl + elh + ell + fovq) & 255;

        int b = px.x == 0 ? 199
              : px.x == 1 ? 99
              : px.x == 2 ? azh
              : px.x == 3 ? azl
              : px.x == 4 ? elh
              : px.x == 5 ? ell
              : px.x == 6 ? fovq
              : chk;
        color.a = float(b) / 255.0;
    }

    fragColor = color;
}
