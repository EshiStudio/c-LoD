#!/usr/bin/env bash
# Launch the real vanilla 26.3 client under Xvfb and auto-join the bench server.
# Usage: run_client.sh   — logs to /tmp/client.log, DISPLAY :99 (1280x720)
set -euo pipefail
C="$HOME/mc-test/client"
JAVA="$HOME/mc-test/jdk25/bin/java"

pkill -f "net.minecraft.client.main.Main" 2>/dev/null || true
pkill Xvfb 2>/dev/null || true
sleep 1

Xvfb :99 -screen 0 1280x720x24 +extension GLX +render -noreset > /tmp/xvfb.log 2>&1 &
XVFB=$!
sleep 2

export DISPLAY=:99
export LIBGL_ALWAYS_SOFTWARE=1
export MESA_GL_VERSION_OVERRIDE=4.5
# glvnd in this container must route GLX to the mesa vendor (swrast/llvmpipe)
export __GLX_VENDOR_LIBRARY_NAME=mesa
export GLX_VENDOR_LIBRARY_NAME=mesa
# sRGB-less GLX shim: renderpearl asks for SDL_GL_FRAMEBUFFER_SRGB_CAPABLE=1,
# which dril's GLX cannot satisfy - the shim strips that attrib (see scripts/bench/libclodgl.c)
export LD_LIBRARY_PATH="$HOME/mc-test/shim${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

cd "$C"
mkdir -p screenshots

# 26.3 defaults to the Vulkan renderpearl backend — force OpenGL (llvmpipe).
# Enum PreferredGraphicsApi serializes as default/opengl/vulkan.
OPT="$C/options.txt"
touch "$OPT"; chmod u+w "$OPT"
grep -q "^preferredGraphicsBackend:" "$OPT" && sed -i 's/^preferredGraphicsBackend:.*/preferredGraphicsBackend:opengl/' "$OPT" || echo "preferredGraphicsBackend:opengl" >> "$OPT"
grep -q "^pauseOnLostFocus:" "$OPT" && sed -i 's/^pauseOnLostFocus:.*/pauseOnLostFocus:false/' "$OPT" || echo "pauseOnLostFocus:false" >> "$OPT"
grep -q "^renderDistance:" "$OPT" && sed -i 's/^renderDistance:.*/renderDistance:4/' "$OPT" || echo "renderDistance:4" >> "$OPT"
grep -q "^fullscreen:" "$OPT" && sed -i 's/^fullscreen:.*/fullscreen:false/' "$OPT" || echo "fullscreen:false" >> "$OPT"
grep -q "^graphicsMode:" "$OPT" && sed -i 's/^graphicsMode:.*/graphicsMode:0/' "$OPT" || echo "graphicsMode:0" >> "$OPT"
grep -q "^mipmapLevels:" "$OPT" && sed -i 's/^mipmapLevels:.*/mipmapLevels:0/' "$OPT" || echo "mipmapLevels:0" >> "$OPT"
grep -q "^entityShadows:" "$OPT" && sed -i 's/^entityShadows:.*/entityShadows:false/' "$OPT" || echo "entityShadows:false" >> "$OPT"
grep -q "^onboardAccessibility:" "$OPT" && sed -i 's/^onboardAccessibility:.*/onboardAccessibility:false/' "$OPT" || echo "onboardAccessibility:false" >> "$OPT"
grep -q "^tutorialStep:" "$OPT" && sed -i 's/^tutorialStep:.*/tutorialStep:none/' "$OPT" || echo "tutorialStep:none" >> "$OPT"
grep -q "^biomeBlendRadius:" "$OPT" && sed -i 's/^biomeBlendRadius:.*/biomeBlendRadius:0/' "$OPT" || echo "biomeBlendRadius:0" >> "$OPT"
grep -q "^simulationDistance:" "$OPT" && sed -i 's/^simulationDistance:.*/simulationDistance:4/' "$OPT" || echo "simulationDistance:4" >> "$OPT"

export LP_NUM_THREADS=$(nproc)
nohup "$JAVA" -Xmx1200M \
  -Djava.library.path="$C/natives" \
  -Dorg.lwjgl.librarypath="$C/natives" \
  -Dorg.lwjgl.opengl.libname=libGL.so.1 \
  -cp "$(cat $C/cp.txt)" \
  net.minecraft.client.main.Main \
  --username Stepan \
  --version 26.3 \
  --gameDir "$C" \
  --assetsDir "$C/assets" \
  --assetIndex 34 \
  --accessToken 0 \
  --userType legacy \
  --versionType release \
  --width 640 --height 360 \
  --quickPlayMultiplayer "127.0.0.1:25565" \
  > /tmp/client.log 2>&1 &
CLI=$!
echo "client pid $CLI (xvfb $XVFB)"
