#version 330
#extension GL_ARB_separate_shader_objects : require

#include <minecraft:fog.glsl>
#include <minecraft:globals.glsl>
#include <minecraft:projection.glsl>
#include <minecraft:sample_lightmap.glsl>
#include <minecraft:terrainglobals.glsl>
#ifndef MULTIDRAW_TERRAIN
    #include <minecraft:chunksection.glsl>
#endif

layout(location = 0) in vec3 Position;
layout(location = 1) in vec4 Color;
layout(location = 2) in vec2 UV0;
layout(location = 3) in ivec2 UV2;
#ifdef MULTIDRAW_TERRAIN
layout(location = 4) in ivec3 ChunkPosition;
layout(location = 5) in float ChunkVisibility;
#endif

#ifndef OIT_ALPHA_ONLY
uniform sampler2D Sampler2;
#endif

layout(location = 0) out float sphericalVertexDistance;
layout(location = 1) out float cylindricalVertexDistance;
layout(location = 2) out vec4 vertexColor;
layout(location = 3) out vec2 texCoord0;
layout(location = 4) out float chunkVisibility;

void main() {
    vec3 pos = Position + (ChunkPosition - CameraBlockPos) + CameraOffset;
    gl_Position = ProjMat * ModelViewMat * vec4(pos, 1.0);

    // c-LoD: distance fog on the real chunks is disabled -- the LOD pass paints
    // the distance instead, and vanilla's fog (which ends at the server view
    // distance) would otherwise leave a white ring right where the LOD starts.
    // apply_fog() returns 0 fog for distance 0, so both terms are zeroed here.
    sphericalVertexDistance = 0.0;
    cylindricalVertexDistance = 0.0;
    #ifndef OIT_ALPHA_ONLY
    vertexColor = Color * sample_lightmap(Sampler2, UV2);
    #else
    vertexColor = Color;
    #endif
    texCoord0 = UV0;

    // c-LoD: kill vanilla's per-section fade-in. It blends new chunk sections
    // from FogColor (a white-ish flash on every section that streams in), which
    // visibly pulses against the LOD band behind them. Sections are simply
    // opaque as soon as they are drawn.
    chunkVisibility = 1.0;
}
