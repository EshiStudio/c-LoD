#!/usr/bin/env bash
# One-shot e2e: (re)use a live finalized server or boot one, then client shots.
# Designed to run inside a single tool call so the sandbox reaper can't
# kill the session mid-flight. Trap guarantees pipe-cleanup on ANY exit.
set -u
S=/home/z/my-project/scripts/bench

cleanup() {
  pkill -f "net.minecraft.client.main.Main" 2>/dev/null
  pkill -f launcher 2>/dev/null
  pkill -f paper-26.3 2>/dev/null
  pkill Xvfb 2>/dev/null
  pkill -f "tail -f /tmp/server.in" 2>/dev/null
  return 0
}
trap cleanup EXIT

cleanup
sleep 1

if grep -q "pack finalized (ONE-TIME, static)" /tmp/server.log 2>/dev/null \
   && ps aux | grep "[p]aper-26.3" > /dev/null 2>&1; then
  echo "reusing live finalized server"
else
  bash "$S/boot_server.sh" || { echo "BOOT FAILED"; exit 1; }
fi

bash "$S/run_e2e_shots.sh" || { echo "CLIENT PHASE FAILED"; exit 2; }

echo "FULL E2E COMPLETE $(date +%H:%M:%S)"
