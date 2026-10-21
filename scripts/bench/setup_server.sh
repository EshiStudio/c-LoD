#!/usr/bin/env bash
# Assemble the CRUSSTY dist root for the c-LoD bench (mirrors CI e2e job).
set -euo pipefail
D="$HOME/mc-test/dist"
CLOD=/home/z/my-project/c-LoD
CRUSSTY=/home/z/my-project/analysis/crussty

rm -rf "$D"
mkdir -p "$D/versions" "$D/modules/lod"
cp "$HOME/mc-test/dl/paper-26.3.jar" "$D/versions/paper-26.3.jar"
cp "$CLOD/target/release/liblod.so" "$D/modules/lod/"
cp "$CLOD/module.json" "$D/modules/lod/"

export PATH="$HOME/.cargo/bin:$PATH"
if [ ! -f "$CRUSSTY/runtime/target/release/libcrussty_runtime.so" ]; then
  (cd "$CRUSSTY" && cargo build --release --manifest-path runtime/Cargo.toml)
fi
cp "$CRUSSTY/runtime/target/release/libcrussty_runtime.so" "$D/"

if [ ! -f "$D/launcher.jar" ]; then
  rm -rf /tmp/lout && mkdir -p /tmp/lout "$D"
  "$HOME/mc-test/jdk25/bin/javac" -d /tmp/lout "$CRUSSTY/launcher/src/main/java/dev/dist/launcher/Main.java"
  "$HOME/mc-test/jdk25/bin/jar" cfe "$D/launcher.jar" dev.dist.launcher.Main -C /tmp/lout .
fi

printf 'eula=true\n' > "$D/eula.txt"
{
  echo 'online-mode=false'
  echo 'view-distance=4'
  echo 'simulation-distance=4'
  echo 'server-port=25565'
  echo 'spawn-protection=0'
  echo 'white-list=false'
  echo 'motd=c-LoD bench'
} > "$D/server.properties"

echo "dist root ready at $D"
