# c-LoD

**True server-side level-of-distant-detail terrain for Minecraft Java 26.3 ("Wilderness Bound") on Paper — as a [CRUSSTY](https://github.com/PLANETA9091/CRUSSTY) module written in Rust.**

No client mods. **No Display entities.** The vanilla client renders the distant world itself, driven entirely by the `/posteffect` command + a server resource pack.

```
 ┌─────────────────────────── SERVER (c-LoD module, Rust) ───────────────────────────┐
 │                                                                                   │
 │  heightmap scan (JNI, budgeted per tick) ──► atlas (height/water grid)            │
 │        │                                        │                                 │
 │        ▼                                        ▼                                 │
 │  PlayerPostEffects.add(Key)      resource pack: post_effect chains + GLSL         │
 │  Player.setResourcePack(...) ◄── + atlas PNG, served by embedded HTTP server       │
 └────────────┬─────────────────────────────────────────────────────┬────────────────┘
              │  ClientboundPostEffectsPacket                        │  pack download
              ▼                                                      ▼
 ┌──────────────────────────── VANILLA CLIENT 26.3 ──────────────────────────────────┐
 │  CAMERA CHANNEL (the v4.0 sun tracker is gone):                                    │
 │  sky.fsh override — vanilla sky program + payload: the sky stage is the only       │
 │  one holding the true camera rotation (ModelViewMat) and FOV (ProjMat[1][1]);      │
 │  it encodes az/el/fov into the ALPHA of the leftmost 8 sky columns (RGB is         │
 │  untouched — the channel is invisible on screen).                                  │
 │                                                                                    │
 │  clod:lod_terrain  (5 passes, every frame)                                         │
 │   1. col_scan    — 640x1 micro-pass: scan the column top-down for the topmost      │
 │                    intact payload row (magic + checksum); terrain alpha=1.0,       │
 │                    clouds/particles/weather fail it -> the channel self-heals      │
 │   2. rot_solve   — 1x1: decode (az, el, fov); confidence 1.0 on any hit,           │
 │                    0.99/frame decay only while the column is fully occluded        │
 │   3. rot_copy    — ping-pong the rotation target                                   │
 │   4. lod_terrain — raymarch the heightfield atlas over sky pixels: distant         │
 │                    mountains, water, snow, distance fog into the veil color        │
 │   5. blit        — composite back into minecraft:main                              │
 │  clod:atmosphere  — depth veil over real terrain: hides the render-distance edge   │
 │                    (depth-only, never needs rotation)                              │
 └────────────────────────────────────────────────────────────────────────────────────┘
```

## Why this works on a vanilla client

Every piece below was verified against the **unobfuscated 26.3 client jar** (Mojang maps are shipped raw since 26.3):

| Fact | Source |
|------|--------|
| `ClientboundPostEffectsPacket(List<Identifier>)` → `LocalPlayer.setActivePostEffects` → `GameRenderer` diffs and runs `PostChain.process(...)` every frame | `ClientCommonPacketListenerImpl`, `GameRenderer`, `net/minecraft/client/renderer/PostChain` |
| Chains load from the server resource pack: `assets/<ns>/post_effect/*.json` + `assets/<ns>/shaders/post/*.fsh` | `PostChainConfig`, vanilla `invert.json` / `entity_outline.json` |
| Post passes can sample the **depth buffer** of `minecraft:main` (`"use_depth_buffer": true`) | `PostChainConfig$TargetInput`, `PostPass$TargetInput.getDepthTextureView` |
| Post shaders get the **camera world position** via the `Globals` UBO (`CameraBlockPos` + `CameraOffset`), plus `GameTime` | `include/globals.glsl`, consumer `post/box_blur.fsh` (proves Globals is bound in post passes) |
| Pack textures are addressable as samplers (`texture: "clod:effect/atlas"` → `clod:textures/effect/atlas.png`) | `PostChainConfig$TextureInput` (`sampler_name/location/width/height/bilinear`), `PostChain` `withPath` |
| Core shaders resolve through the resource system → a server pack can override the **vanilla sky shader** (`pipeline/sky` → shader ids `core/sky`) | `RenderPipelines` (pipeline builder), `ShaderManager` resource loading |
| The sky pipeline binds `Projection` + `DynamicTransforms` UBOs and has **no blending** (`ColorTargetState.DEFAULT`, writeMask 15) — raw RGBA (alpha included) lands in `minecraft:main` | `RenderPipelines` bytecode, `ColorTargetState` |
| Uniform types `float/int/ivec3/vec2/vec3/vec4/matrix4x4` become `std140` UBOs from JSON | `UniformValue$Type` |
| Paper API: `Player#postEffects()` → `PlayerPostEffects.add/remove(Key)`, `Player.setResourcePack(UUID,url,sha1,prompt,force)` | Paper 26.3 javadoc |

The sky region is `depth == 1.0` (sky/clouds never write depth), which is exactly where the LOD raymarch paints. The **view basis travels through the frame itself**: the sky shader computes `yaw/pitch/fov` from `ModelViewMat`/`ProjMat` and packs them into the invisible alpha of the leftmost sky columns; the chain scans for the topmost intact row (magic + checksum), so any occlusion self-heals frame-exact — **no sun, no moon, no time-of-day requirement, no warm-up**, and the real per-client FOV (spyglass zoom included) rides along for free.

## Install

```bash
crussty install PLANETA9091/c-LoD
# or drop the release bundle into modules/
```

Requires: Paper (or Purpur) 26.3+, Java 25+ runtime, CRUSSTY runtime. The module opens one HTTP port for the resource pack.

## Configuration

Options string passed through the CRUSSTY runtime (`key=value;key=value`):

| Key | Default | Meaning |
|-----|---------|---------|
| `http_port` | `8199` | Port of the embedded pack server |
| `pack_base_url` | *(auto)* | Public base URL, e.g. `http://play.example.com` |
| `pack_uuid` | fixed | Stable pack UUID (clients re-download only on SHA-1 change) |
| `pack_prompt` | *(empty)* | Client prompt text |
| `world` | first world | World scanned for the atlas |
| `center_x` / `center_z` | `0` | Atlas center in blocks |
| `atlas_size` | `4096` | Side length in blocks (power of two, 512–16384) |
| `cell_size` | `4` | Blocks per atlas texel (power of two) |
| `scan_budget` | `4096` | Heightmap cells scanned per engine tick |
| `scan_deadline_secs` | `900` | Static-pack scan deadline: the pack is built EXACTLY ONCE when the sweep completes or this fires — never rebuilt, never re-pushed |
| `atlas_cache` | `true` | Persist the finalized atlas to `atlas_path` so restarts reproduce byte-identical packs (same SHA-1 — clients never re-download) |
| `atlas_path` | `c-LoD-atlas.bin` | Where the atlas snapshot is persisted |
| `tick_ms` | `1000` | Engine tick interval (scan + per-player tier switching) |
| `fov` | `70` | Static fallback only — v4.1 measures the real per-client FOV in the sky shader (spyglass zoom works) |
| `lod_near` | `96` | LOD raymarch starts here (keep BELOW the client view distance: real chunks win by depth, LOD fills underneath the chunk edge — no seam) |
| `sea_level` | `63` | Water level |
| `color_low/color_high/color_water/color_veil` | defaults | Palette, `r/g/b` floats or 0–255 |
| `snow_line` | `0.78` | Snow blend point (normalized elevation) |
| `max_height` | `320` | Terrain ceiling used for ray culling |

Post-effect ids installed by the pack: **`clod:lod_terrain`**, **`clod:atmosphere`** — exactly the identifiers the vanilla `/posteffect` command accepts, so admins can also toggle them by hand:

```
/posteffect add @a clod:lod_terrain
```

## Development

```bash
bash build.sh                 # cargo build --release -> liblod.so
cargo test                    # 30 unit tests (zip/png/sha1/atlas/packgen/config/engine)
bash scripts/check_glsl.sh    # glslangValidator over all shaders + chain JSONs
```

CI runs rustfmt, clippy (`-D warnings`), the full test suite, glslang shader validation, release builds on Linux/macOS/Windows, the CRUSSTY module bundle — and an **e2e live-server job**: a real Paper 26.3 kernel on Temurin 25 with the CRUSSTY runtime built from source, two boots asserting gates G1–G7 (single static finalize, zero runtime rebuilds, HTTP pack fetch, `atlas restored` + SHA-1 stability across restarts).

## Honest limitations (v4.1)

- **The camera channel needs sky in the leftmost screen columns.** Facing a wall of terrain (or being fully indoors) the chain holds its last estimate with a slow decay — the LOD fades out instead of popping, and the atmosphere chain keeps working (it never needs rotation). Any sky row in the leftmost 8 columns re-locks the basis within one frame.
- Overdraw on those columns (clouds, rain, particles) fails the payload checksum per row and simply redirects the scan lower — self-healing by design; a fully overcast frame may drop the LOD briefly.
- Atlas generation forces chunk loads — keep `scan_budget` reasonable for your tick budget.

## License

MIT
