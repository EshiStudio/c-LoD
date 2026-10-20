#!/usr/bin/env bash
# Client e2e phase: launch vanilla client, auto-join, screenshot matrix.
# Requires: server already booted and pack finalized (boot_server.sh).
set -uo pipefail
SHOTS="$HOME/mc-test/shots"
mkdir -p "$SHOTS"

srv() { echo "$1" > /tmp/server.in; }

wait_for() { # pattern, file, timeout_s
  for i in $(seq 1 "$3"); do
    grep -q "$1" "$2" 2>/dev/null && return 0
    sleep 1
  done
  return 1
}

# 1. server sanity
wait_for "pack finalized (ONE-TIME, static)" /tmp/server.log 5 || { echo "server not finalized"; exit 2; }
ps aux | grep "[p]aper-26.3" > /dev/null || { echo "server process dead"; exit 2; }
curl -s -o /tmp/pack_check.zip -w "pack HTTP %{http_code} %{size_download}B\n" http://127.0.0.1:8199/clod-pack.zip

# 2. client
bash /home/z/my-project/scripts/bench/run_client.sh
wait_for "Stepan joined the game" /tmp/server.log 420 || { echo "client never joined"; tail -20 /tmp/client.log; exit 3; }
echo "client joined"
sleep 5
ffmpeg -y -loglevel error -f x11grab -video_size 1280x720 -framerate 2 -i :99 -frames:v 1 "$SHOTS/00_join_state.png"
echo "join-state shot taken (pack prompt expected)"

# the server pack prompt is on screen — click Yes (window 640x360 centered on
# 1280x720 Xvfb: Yes button spans x 489..635, y 376..394 -> center ~562,385)
python3 /home/z/my-project/scripts/bench/click.py 562 385
sleep 12
ffmpeg -y -loglevel error -f x11grab -video_size 1280x720 -framerate 2 -i :99 -frames:v 1 "$SHOTS/00_pack_applied.png"
echo "pack-accept clicked"

# 3. world state: lock NOON (the v4.0-killer scenario) + clear weather
srv "gamerule doDaylightCycle false"
srv "time set 6000"
srv "weather clear"
srv "gamemode spectator Stepan"
sleep 2

shot() { # name yaw pitch x y z
  srv "tp Stepan $4 $5 $6 $2 $3"
  sleep 3
  ffmpeg -y -loglevel error -f x11grab -video_size 1280x720 -framerate 2 -i :99 -frames:v 1 "$SHOTS/$1.png"
  echo "shot $1 (yaw=$2 pitch=$3 at $4,$5,$6)"
}

shot 01_noon_ground  90  0    0 100 0
shot 02_noon_high    90  5    0 120 0
shot 03_noon_aerial  90  15   0 160 0
shot 04_yaw_south     0  5    0 130 0
shot 05_yaw_north   180  5    0 130 0
shot 06_yaw_east    270  5    0 130 0
shot 07_veil_outside 90 10  700 140 0
shot 08_back_inside  90  0    0 120 0

echo "=== client log pack markers ==="
rg -N -i "pack|post" /tmp/client.log | tail -12 || true
pkill -f "net.minecraft.client.main.Main" 2>/dev/null || true
echo "E2E CLIENT PHASE DONE"
