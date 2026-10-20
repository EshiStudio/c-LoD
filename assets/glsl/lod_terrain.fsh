#version 330
#extension GL_ARB_separate_shader_objects : require
#include <minecraft:globals.glsl>

// c-LoD pass 5: the actual level-of-distant-detail renderer.
//
// For every pixel that shows sky (device depth ~ 1) we march a view ray
// against the heightfield atlas shipped in the server resource pack and
// shade the hit. The ray is built from the camera basis recovered by the
// sun tracker (clod_rot target) and the configured FOV. Camera position
// comes from the Globals UBO (CameraBlockPos + CameraOffset), which the
// engine binds to every post pass — verified against vanilla box_blur.fsh.
//
// Non-sky pixels pass through untouched: real chunks always win.

uniform sampler2D In;      // minecraft:main color
uniform sampler2D InDepth; // minecraft:main depth
uniform sampler2D Atlas;   // clod:textures/effect/atlas.png (RG = height, B = water)
uniform sampler2D Rot;     // clod:clod_rot_a (1x1 persistent)

layout(std140) uniform SamplerInfo {
    vec2 OutSize;
    vec2 InSize;
};

// x,y = atlas origin (blocks), z = atlas size (blocks), w = cell size (blocks)
layout(std140) uniform ClodAtlas {
    vec4 AtlasInfo;
};

// x = lod near (blocks), y = max terrain height, z = sea level, w = min confidence
layout(std140) uniform ClodParams {
    vec4 LodParams;
};

// x = fog start (blocks), y = fog end (blocks), z = fov (deg), w = unused
layout(std140) uniform ClodFog {
    vec4 FogParams;
};

layout(std140) uniform ClodColors {
    vec4 ColLow;    // low terrain
    vec4 ColHigh;   // high terrain
    vec4 ColWater;  // water surface
    vec4 VeilColor; // horizon veil (rgb) + snow whiteness (a)
};

layout(location = 0) in vec2 texCoord;
layout(location = 0) out vec4 fragColor;

const float PI = 3.14159265358979;
const float SKY_DEPTH = 0.99995;
const int MAX_STEPS = 220;
const int BISECT = 7;

// Height (block Y) stored in the atlas: (y + 64) << 6, big-endian RG.
float atlas_height(vec2 uv) {
    vec4 a = texture(Atlas, uv);
    float y16 = a.r * 65280.0 + a.g * 255.0; // r*255*256 + g*255
    return y16 / 64.0 - 64.0;
}

bool atlas_water(vec2 uv) {
    return texture(Atlas, uv).b > 0.5;
}

// Heightfield lookup in world space; clamps to the atlas edge (plateau).
float world_height(vec2 xz, vec2 origin, float size) {
    vec2 uv = (xz - origin) / size;
    uv = clamp(uv, vec2(0.0015), vec2(0.9985));
    return atlas_height(uv);
}

bool world_water(vec2 xz, vec2 origin, float size) {
    vec2 uv = (xz - origin) / size;
    uv = clamp(uv, vec2(0.0015), vec2(0.9985));
    return atlas_water(uv);
}

void main() {
    vec4 scene = texture(In, texCoord);
    float depth = texture(InDepth, texCoord).r;

    if (depth < SKY_DEPTH) {
        fragColor = scene;
        return;
    }

    vec4 rot = texelFetch(Rot, ivec2(0), 0);
    float conf = rot.b;
    float minConf = LodParams.w;
    if (conf < minConf) {
        fragColor = scene;
        return;
    }

    float az = rot.r * 6.28318530718;
    float el = rot.g * PI - PI * 0.5;

    vec3 F = vec3(cos(el) * cos(az), sin(el), cos(el) * sin(az));
    vec3 R = vec3(-sin(az), 0.0, cos(az));
    vec3 U = vec3(-cos(az) * sin(el), cos(el), -sin(az) * sin(el));

    float fov = FogParams.z;
    float tanH = tan(radians(fov) * 0.5);
    float aspect = OutSize.x / max(OutSize.y, 1.0);

    vec2 ndc = texCoord * 2.0 - 1.0;
    vec3 dir = normalize(F + R * (ndc.x * tanH * aspect) + U * (ndc.y * tanH));

    vec3 cam = vec3(CameraBlockPos) + CameraOffset;

    vec2 origin = AtlasInfo.xy;
    float size = AtlasInfo.z;
    float cell = AtlasInfo.w;
    float near = LodParams.x;
    float maxY = LodParams.y;
    float sea = LodParams.z;

    // The atlas is a square around `origin`; stop the ray when it leaves.
    vec2 minW = origin - vec2(size * 0.5);
    vec2 maxW = origin + vec2(size * 0.5);

    // Rays that leave the heightfield from above can never hit: early-out.
    if (dir.y > 0.0) {
        float toCeil = (maxY + 64.0 - cam.y) / dir.y;
        float toEdge = 1.0e9;
        if (dir.x > 0.0) toEdge = min(toEdge, (maxW.x - cam.x) / dir.x);
        if (dir.x < 0.0) toEdge = min(toEdge, (minW.x - cam.x) / dir.x);
        if (dir.z > 0.0) toEdge = min(toEdge, (maxW.y - cam.z) / dir.z);
        if (dir.z < 0.0) toEdge = min(toEdge, (minW.y - cam.z) / dir.z);
        if (toCeil < toEdge) {
            fragColor = scene; // leaves the heightfield from above: sky
            return;
        }
    }
    if (dir.y == 0.0) {
        dir.y = 1.0e-6;
    }

    // March: start at the LOD near edge (real chunks live inside it), step
    // proportional to screen-space footprint of one atlas cell.
    float t = near;
    float dt = max(cell * 0.75, t * (2.0 * tanH / max(OutSize.y, 1.0)));
    float tHit = -1.0;
    float prevT = t;
    float prevDiff = 1.0e9;

    vec2 xz = cam.xz + dir.xz * t;
    float above = (world_water(xz, origin, size) ? sea : world_height(xz, origin, size)) - (cam.y + dir.y * t);

    for (int i = 0; i < MAX_STEPS; ++i) {
        prevT = t;
        prevDiff = above;
        t = min(t + dt, size * 0.75);
        dt = max(cell * 0.75, t * (2.0 * tanH / max(OutSize.y, 1.0)));

        xz = cam.xz + dir.xz * t;
        // Left the atlas? No LOD here.
        if (any(lessThan(xz, minW)) || any(greaterThan(xz, maxW))) {
            break;
        }
        float ground = world_water(xz, origin, size) ? sea : world_height(xz, origin, size);
        above = ground - (cam.y + dir.y * t);

        if (above <= 0.0 && prevDiff > 0.0) {
            // Bisection between prevT and t.
            float lo = prevT;
            float hi = t;
            for (int b = 0; b < BISECT; ++b) {
                float mid = (lo + hi) * 0.5;
                vec2 mxz = cam.xz + dir.xz * mid;
                float g = world_water(mxz, origin, size) ? sea : world_height(mxz, origin, size);
                if (g - (cam.y + dir.y * mid) <= 0.0) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            tHit = (lo + hi) * 0.5;
            break;
        }
        if (t >= size * 0.75) {
            break;
        }
    }

    if (tHit < 0.0) {
        fragColor = scene; // open sky (or out of atlas): vanilla wins
        return;
    }

    float fade = smoothstep(minConf, minConf + 0.15, conf);
    vec3 hitPos = cam + dir * tHit;

    // Shade: normal from the height gradient, palette by elevation.
    vec2 e = vec2(cell, 0.0);
    float hL = world_height(hitPos.xz - e.xy, origin, size);
    float hR = world_height(hitPos.xz + e.xy, origin, size);
    float hD = world_height(hitPos.xz - e.yx, origin, size);
    float hU = world_height(hitPos.xz + e.yx, origin, size);
    vec3 n = normalize(vec3(hL - hR, 2.0 * cell, hD - hU));

    float tt = fract(GameTime) * 6.28318530718;
    vec3 sunDir = vec3(cos(tt), sin(tt), 0.0);
    // RAW sun elevation drives the day cycle (shader-pack practice: distant
    // detail must dissolve into a color that tracks the sky, not a constant).
    // Post chains get no Fog UBO (verified in 26.3 sources: no vanilla post
    // shader binds one), so GameTime is the only live sky data available.
    float sunY = sin(tt);
    float dayF = smoothstep(-0.14, 0.22, sunY);
    vec3 veilCol = VeilColor.rgb * mix(0.42, 1.0, dayF);
    veilCol = mix(veilCol * vec3(0.85, 0.9, 1.12), veilCol, dayF); // moonlit blue at night
    if (sunDir.y < -0.12) {
        sunDir = -sunDir;
    }

    float hn = clamp((hitPos.y - sea) / max(maxY - sea, 1.0), 0.0, 1.0);
    vec3 terrainCol = mix(ColLow.rgb, ColHigh.rgb, smoothstep(0.15, 0.9, hn));
    float snow = smoothstep(VeilColor.a * 0.55 + 0.35, VeilColor.a * 0.55 + 0.55, hn);
    terrainCol = mix(terrainCol, vec3(0.93, 0.95, 0.97), snow);

    float isWater = world_water(hitPos.xz, origin, size) ? 1.0 : 0.0;
    vec3 albedo = mix(terrainCol, ColWater.rgb, isWater);

    vec3 lightDir = normalize(vec3(sunDir.x, max(sunDir.y, 0.15), 0.0));
    float diff = max(dot(n, lightDir), 0.0);
    float ambient = mix(0.20, 0.55, dayF);
    float sunGain = mix(0.30, 0.80, dayF);
    vec3 lit = albedo * (ambient + diff * sunGain);

    // Distance fog into the day-graded veil color: merges LOD into the sky.
    float fog = smoothstep(FogParams.x, FogParams.y, tHit);
    lit = mix(lit, veilCol, fog);

    fragColor = vec4(mix(scene.rgb, lit, fade), scene.a);
}
