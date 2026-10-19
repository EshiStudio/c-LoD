# c-LoD

**Level-of-Distant-Detail terrain for Paper 26.3+ servers.** A
[CRUSSTY](https://github.com/PLANETA9091/CRUSSTY) module written in pure Rust
that renders the far world as block-impostor terrain — **no client mods
required**, works on the vanilla Minecraft client.

The server plans a quadtree of LOD cells around every player (starting right
outside the vanilla view distance), keeps each cell's chunk loaded via NMS
chunk tickets, snapshots its top terrain, and renders it as an interpolated
`BlockDisplay` impostor. Far mountains, oceans and forests become visible
horizon geometry instead of void.

```
        vanilla view distance          c-LoD impostors (up to max_distance)
   ┌───────────────────────────┬──────────────────────────────────────────┐
   │   real chunks (server)    │   2x2 .. 64x64-chunk impostor boxes      │
   │        8 chunks           │   quadtree annuli, hysteresis            │
   └───────────────────────────┴──────────────────────────────────────────┘
```

## Install

Requires a CRUSSTY-injected server (`crussty init && crussty run`).

```bash
# from a GitHub release bundle
crussty install PLANETA9091/c-LoD

# or drop the module folder next to your other modules
modules/
└── lod/
    ├── module.json
    └── liblod.so
```

Then just boot the server. On first boot the module raises
`entity-tracking-range.display` in `spigot.yml` (pre-kernel, so no restart is
needed) — far displays would be invisible without it.

## Configuration — `modules/lod/clod.conf`

Every key is optional; defaults below.

```ini
max_distance        = 1024   # LOD radius in blocks (128..32768)
view_distance       = 0      # 0 = read live from server.properties
growth              = 2.0    # annulus growth per level (1.3..4.0)
hysteresis          = 0.05   # boundary stickiness (0..0.3)
update_interval_ms  = 500    # planning poll interval

snapshots_per_cycle = 128    # chunk snapshot reads / cycle
spawns_per_cycle    = 64     # display spawns / cycle
updates_per_cycle   = 64     # display updates / cycle
despawns_per_cycle  = 128
pins_per_cycle      = 32     # chunk tickets / cycle
unpins_per_cycle    = 64
max_cells_total     = 4096   # global impostor cap
max_cells_per_player = 512

interpolation_ticks = 8      # display interpolation (0..59)
teleport_ticks      = 8
brightness          = 15     # block+sky light override
refresh_secs        = 60     # re-snapshot active LOD0 cells (world edits)

tickets             = auto   # auto | nms | forceload | none
tracking_raise      = true   # pre-boot spigot.yml entity-tracking-range fix
log_interval_cycles = 40
```

## Chunk pinning backends

| backend     | how                                                  | ticking | persistent |
|-------------|------------------------------------------------------|---------|------------|
| `nms`       | `TicketStorage.addTicketWithRadius(PLAYER_LOADING, pos, 0)` at FULL level (runtime-probed against the 26.3 API) | no  | no  |
| `forceload` | `World#setChunkForceLoaded` (Bukkit)                 | yes | yes |
| `none`      | never pin — impostors only inside loaded chunks      | —   | —   |

`auto` probes the NMS chain and falls back to `forceload`.

## How it works

1. **Pre-boot** (`cplugin_init`, before the kernel): load `clod.conf`, patch
   `spigot.yml`.
2. **Kernel-ready**: resolve the whole Bukkit + JOML + NMS surface once on the
   main thread (lazy classes forced through the kernel class loader,
   fully initialized before `GetMethodID`), probe the ticket backend, report
   `bootstrap ok`.
3. **Worker thread** (pure Rust): per player — quadtree annulus selection
   (cells of 2x2..64x64 chunks, distance bands `[T_L, T_{L+1})` with
   hysteresis), snapshot aggregation from the chunk cache, delta planning
   (spawn/update/despawn/pin/unpin) under per-cycle budgets.
4. **Main thread**: execute op batches — spawn `BlockDisplay` impostors
   (scale = cell size, height = terrain, map-color representative block,
   brightness 15, interpolation), pin/unpin chunks via `TicketStorage`.

All NMS signatures are verified at runtime with precise diagnostics — no
silent guessing. Every JVM failure degrades gracefully (missing entities
respawn, failed pins retry).

## Build

```bash
cargo build --release        # -> target/release/liblod.so
cargo test                   # 39 unit + integration tests
cargo clippy --all-targets -- -D warnings
```

CI runs fmt + clippy + tests + a real **Paper 26.3 boot e2e** on GitHub
Actions (see `.github/workflows/ci.yml`).

## License

MIT
