# Real-client E2E bench (local harness)

The CI `e2e-live` job boots the server stack and asserts gates G1–G7, but only
a real vanilla 26.3 client can prove the *visual* half: pack acceptance, the
post-chain compiling, and the camera channel holding at noon (the exact v4.0
killer scenario). This directory contains the full local harness that does
exactly that. It was used to verify v4.1.0 on a genuine vanilla 26.3 client —
see `docs/e2e-2026-10-20-noon/` for the screenshot matrix.

## What's here

| file | role |
|------|------|
| `setup_client.py` | rebuilds the vanilla 26.3 client runtime from the version manifest (66 libraries + natives extraction + 5147 asset objects) |
| `setup_server.sh` | assembles the CRUSSTY dist root (modules/lod + launcher + runtime + Paper 26.3), mirrors the CI job |
| `boot_server.sh` | boots the stack with a stdin fifo (`/tmp/server.in`), waits for the one-time pack finalize, runs gates G1–G7 |
| `run_client.sh` | Xvfb :99 + vanilla client with `--quickPlayMultiplayer`, forced `preferredGraphicsBackend:opengl`, fast llvmpipe options |
| `libclodgl.c` | **the sRGB shim** — see below |
| `press_key.py` / `click.py` | XTest key/click injection (accepts the server resource-pack prompt headlessly) |
| `run_e2e_shots.sh` | join → accept pack → lock noon → 8-position screenshot matrix (4 yaws + aerial + Veil-outside + back-inside) |
| `run_full_e2e.sh` | one-shot supervisor: boot (or reuse a live finalized server) + client phase + guaranteed cleanup |

## The sRGB shim (why libclodgl.c exists)

Minecraft 26.3's renderpearl defaults to Vulkan and falls back to
`PreferredGraphicsApi.opengl` — the OpenGL backend asks SDL for
`SDL_GL_FRAMEBUFFER_SRGB_CAPABLE=1`. Mesa's dril GLX (Debian 13, used by
Xvfb) exposes **no sRGB-capable visuals**, so `glXChooseFBConfig` matches
nothing and the client dies with `Couldn't find matching GLX visual`.

`libclodgl.c` builds `~/mc-test/shim/libGL.so.1`: SDL dlopens it through
`LD_LIBRARY_PATH`; every symbol forwards to the real libGL, except
`glXChooseFBConfig` / `glXChooseVisual`, which strip the
`GLX_FRAMEBUFFER_SRGB_CAPABLE` (0x20B1/0x20B2) pair — and
`glXGetProcAddress(ARB)`, which hands out the filtered versions. Software
sRGB-less configs are perfectly fine for a test bench.

## Local memory budget (4 GB container)

Paper runs with `-Xms256M -Xmx700M -XX:MaxMetaspaceSize=256M
-XX:MaxDirectMemorySize=128M` (via `DIST_JAVA_OPTS`), atlas `512`, scan
deadline 45 s; the client runs `-Xmx1200M` at 640×360 with llvmpipe
(`LP_NUM_THREADS=$(nproc)`). The 43 % atlas coverage from the short scan is
deliberate — the deterministic SHA-1 across restarts (`atlas restored`) is
what the run proves.

## Reproduce

```bash
python3 scripts/bench/setup_client.py          # once, ~600 MB downloads
bash   scripts/bench/setup_server.sh           # once, needs liblod.so + CRUSSTY
bash   scripts/bench/run_full_e2e.sh           # boot + client + shots
```

Screenshots land in `~/mc-test/shots/`.
