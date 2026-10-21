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

uniform sampler2D InSampler;      // minecraft:main color
uniform sampler2D InDepthSampler; // minecraft:main depth
uniform sampler2D AtlasSampler;   // clod:textures/effect/atlas.png (RG = height, B = water)
uniform sampler2D RotSampler;     // clod:clod_rot_a (1x1 persistent)

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
// x = debug mode (0 = normal, 1 = paint confidence into the sky)
layout(std140) uniform ClodDebug {
    vec4 DebugParams;
};

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
// 26.3 device depth: d = near/eyeZ (infinite far) — sky == 0.0, any real
// geometry reads positive, so 1e-6 keeps terrain out to ~50 km classified.
const float SKY_DEPTH = 1.0e-6;
const int MAX_STEPS = 420;
const int BISECT = 7;

// Height (block Y) stored in the atlas: (y + 64) << 6, big-endian RG.
float atlas_height(vec2 uv) {
    vec4 a = texture(AtlasSampler, uv);
    float y16 = a.r * 65280.0 + a.g * 255.0; // r*255*256 + g*255
    return y16 / 64.0 - 64.0;
}

bool atlas_water(vec2 uv) {
    return texture(AtlasSampler, uv).b > 0.5;
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
    vec4 scene = texture(InSampler, texCoord);
    float depth = texture(InDepthSampler, texCoord).r;

    // Debug mode 3: pure geometry probe: cell pattern, viewport edge and four
    // coloured corners over the ENTIRE pass viewport.
    if (DebugParams.x > 2.5 && DebugParams.x < 3.5) {
        vec2 uv = gl_FragCoord.xy / max(OutSize, vec2(1.0));
        vec2 cellId = floor(gl_FragCoord.xy / 64.0);
        vec3 c = vec3(mod(cellId.x, 8.0) / 8.0, mod(cellId.y, 8.0) / 8.0, 0.55);
        if (uv.x < 0.004 || uv.x > 0.996 || uv.y < 0.004 || uv.y > 0.996) {
            c = vec3(1.0, 0.0, 1.0); // magenta border = viewport edge
        }
        if (uv.x < 0.07 && uv.y > 0.93) c = vec3(1.0, 0.0, 0.0); // top-left  red
        if (uv.x > 0.93 && uv.y > 0.93) c = vec3(0.0, 1.0, 0.0); // top-right green
        if (uv.x < 0.07 && uv.y < 0.07) c = vec3(0.0, 0.0, 1.0); // bottom-left  blue
        if (uv.x > 0.93 && uv.y < 0.07) c = vec3(1.0, 1.0, 0.0); // bottom-right yellow
        fragColor = vec4(c, 1.0);
        return;
    }

    if (depth > SKY_DEPTH) {
        fragColor = scene;
        return;
    }
    vec4 rot = texelFetch(RotSampler, ivec2(0), 0);
    float conf = rot.b;
    float minConf = LodParams.w;
    float forceLod = 0.0;

    // Debug mode 4: LOD hit mask + tracker confidence bar.
    //   green     = the ray hit the heightfield (this pixel WOULD get LOD)
    //   orange    = sky, the march ran but never hit
    //   purple    = the ray leaves the heightfield from above (never hits)
    //   bar (top-left, 240x24 px): red width = tracker confidence 0..1
    if (DebugParams.x > 3.5) {
        if (gl_FragCoord.x < 240.0 && gl_FragCoord.y > OutSize.y - 24.0) {
            vec3 bar = gl_FragCoord.x < conf * 240.0 ? vec3(1.0, 0.0, 0.0) : vec3(0.0);
            fragColor = vec4(bar, 1.0);
            return;
        }
        forceLod = 1.0;
    }

    // Debug view: red channel = tracker confidence over sky pixels.
    //   black sky  -> the tracker never locked (sun never detected)
    //   red sky    -> tracker locked (conf == 1)
    //   normal sky -> this pass never classified the pixel as sky
    if (DebugParams.x > 0.5 && DebugParams.x < 1.5) {
        fragColor = vec4(conf, 0.0, 0.0, 1.0);
        return;
    }

    // Debug mode 2 splits the screen:
    //   left  half -> the real ray march with the confidence gate FORCED open
    //   right half -> the atlas itself drawn as a height map (uv = screen uv)
    // so one look tells us whether the march or the atlas data is at fault.
    if (DebugParams.x > 1.5 && DebugParams.x < 2.5) {
        if (texCoord.x >= 0.5) {
            float hm = atlas_height(texCoord);
            float g = clamp((hm + 64.0) / 384.0, 0.0, 1.0);
            vec3 mc = vec3(g);
            if (atlas_water(texCoord)) {
                mc = mix(mc, vec3(0.3, 0.5, 1.0), 0.6);
            }
            fragColor = vec4(mc, 1.0);
            return;
        }
        forceLod = 1.0;
    }

    if (conf < minConf && forceLod < 0.5) {
        fragColor = scene;
        return;
    }

    float az = rot.r * 6.28318530718;
    float el = rot.g * PI - PI * 0.5;

    vec3 F = vec3(cos(el) * cos(az), sin(el), cos(el) * sin(az));
    vec3 R = vec3(-sin(az), 0.0, cos(az));
    vec3 U = vec3(-cos(az) * sin(el), cos(el), -sin(az) * sin(el));

    // FOV from the pose tracker (estimated from the depth buffer, follows the
    // client's real slider and its dynamic sprint/fly offset), pack value as
    // the fallback before the first lock.
    float fov = rot.a > 0.02 ? 30.0 + rot.a * 80.0 : FogParams.z;
    float tanH = tan(radians(fov) * 0.5);
    float aspect = OutSize.x / max(OutSize.y, 1.0);

    vec2 ndc = texCoord * 2.0 - 1.0;
    vec3 dir = normalize(F + R * (ndc.x * tanH * aspect) + U * (ndc.y * tanH));

    // 26.3 Globals: CameraBlockPos = floor(camPos), CameraOffset = floor - camPos
    // (see GlobalSettingsUniform.update), so the camera is floor - offset.
    vec3 cam = vec3(CameraBlockPos) - CameraOffset;

    vec2 origin = AtlasInfo.xy;
    float size = AtlasInfo.z;
    float cell = AtlasInfo.w;
    float near = LodParams.x;
    float maxY = LodParams.y;
    float sea = LodParams.z;

    // The atlas is a square around `origin`; stop the ray when it leaves.
    // AtlasInfo.xy is the MIN corner, z the full side: the box is origin..origin+size.
    vec2 minW = origin;
    vec2 maxW = origin + vec2(size);

    // Rays that leave the heightfield from above can never hit: early-out.
    if (dir.y > 0.0) {
        float toCeil = (maxY + 64.0 - cam.y) / dir.y;
        float toEdge = 1.0e9;
        if (dir.x > 0.0) toEdge = min(toEdge, (maxW.x - cam.x) / dir.x);
        if (dir.x < 0.0) toEdge = min(toEdge, (minW.x - cam.x) / dir.x);
        if (dir.z > 0.0) toEdge = min(toEdge, (maxW.y - cam.z) / dir.z);
        if (dir.z < 0.0) toEdge = min(toEdge, (minW.y - cam.z) / dir.z);
        if (toCeil < toEdge) {
            fragColor = DebugParams.x > 3.5 ? vec4(0.6, 0.0, 0.9, 1.0) : scene;
            return;
        }
    }
    if (dir.y == 0.0) {
        dir.y = 1.0e-6;
    }

    // March from just outside the camera. Starting at `near` was a bug: when the
    // camera looks down, the first sample at `near` is ALREADY below the
    // heightfield (above < 0), so no sign change was ever detected and the whole
    // near band "missed". The pass only writes sky pixels, so the near field is
    // free to start at the camera.
    float t = max(cell, 1.0);
    float dt = max(cell * 0.75, t * 0.004);
    float tHit = -1.0;
    float prevT = t;
    float prevDiff = 1.0e9;

    vec2 xz = cam.xz + dir.xz * t;
    float above = (world_water(xz, origin, size) ? sea : world_height(xz, origin, size)) - (cam.y + dir.y * t);

    for (int i = 0; i < MAX_STEPS; ++i) {
        prevT = t;
        prevDiff = above;
        t = min(t + dt, size);
        // Grow the step so ~420 iterations cover the full atlas (was: constant
        // 2 blocks, which capped the march at ~720 of 2048 blocks).
        dt = max(cell * 0.75, t * 0.004);

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
        if (t >= size) {
            break;
        }
    }

    if (tHit < 0.0) {
        fragColor = DebugParams.x > 3.5 ? vec4(1.0, 0.45, 0.0, 1.0) : scene;
        return;
    }
    if (DebugParams.x > 4.5 && DebugParams.x < 5.5) {
        // Distance tint: red < 300 blocks, green ~600, blue > 900.
        vec3 c = mix(vec3(1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), clamp(tHit / 600.0, 0.0, 1.0));
        c = mix(c, vec3(0.0, 0.25, 1.0), clamp((tHit - 600.0) / 600.0, 0.0, 1.0));
        fragColor = vec4(c, 1.0);
        return;
    }
    if (DebugParams.x > 3.5) {
        fragColor = vec4(0.0, 1.0, 0.1, 1.0);
        return;
    }

    float fade = forceLod > 0.5 ? 1.0 : smoothstep(minConf, minConf + 0.15, conf);
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
