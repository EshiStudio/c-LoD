# Changelog

## 2.1.0 — Adaptive LOD

- **Altitude-adaptive veil tiers**: the atmosphere chain is emitted in
  `atmo_tiers` (default 3) intensity tiers — `clod:atmosphere_t0` (weakest)
  … `t<N-1>` (strongest). Per player, the engine picks the tier from height
  above the scanned terrain each adaptive tick: ground level keeps the full
  render-distance edge mask, elevated players get a weaker veil and a
  crisper horizon. Tier switching happens only on actual changes.
- **Player-following atlas**: when every player in the atlas world has
  drifted `recenter_blocks` (Chebyshev, default 1024) outside the window for
  `recenter_patience` consecutive ticks (default 20), the window recenters
  onto the player centroid, drops its cells and rescans; the pack rebuilds
  and clients re-download by SHA-1.
- New options: `atmo_tiers`, `adaptive`, `recenter_blocks`,
  `recenter_patience`.
- `Atlas` center is now interior-mutable (`RwLock`) so the window can move
  while the atlas lives behind an `Arc`.
- Test suite grown to 32 unit tests (tier bands, adaptive tier selection,
  recenter semantics, distance_outside, pack tier emission).

## 2.0.0 — Posteffect architecture

- True server-side LOD via `/posteffect` + server resource pack. No Display
  entities, no client mods.
- 6-pass `clod:lod_terrain` chain: sun_scan / sun_reduce / sun_solve /
  rot_copy / lod_terrain / blit — raymarches the heightfield atlas over sky
  pixels using the vanilla sun position as the free rotation channel.
- `clod:atmosphere` depth veil hides the render-distance edge.
- JNI heightmap scanning (budgeted per tick), embedded HTTP pack server,
  SHA-1 pack pushes, CRUSSTY module bundle + release automation.

## 1.0.0 — initial public version

- Display-based impostor prototype (superseded by the 2.0 posteffect
  architecture; no Display entities remain).
