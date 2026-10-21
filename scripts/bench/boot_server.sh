#!/usr/bin/env bash
# Boot the Paper 26.3 + CRUSSTY + c-LoD stack; stdin via /tmp/server.in fifo.
# Usage: boot_server.sh   (returns once "pack finalized" appears, gates run)
set -euo pipefail
D="$HOME/mc-test/dist"
JAVA="$HOME/mc-test/jdk25/bin/java"
export CLOD_OPTS="scan_deadline_secs=45;tick_ms=200;atlas_size=512;cell_size=4;http_port=8199"
# 4GB container: Paper+module must fit alongside the client later;
# host OOM-killer targets the fattest RSS — stay small and fast
export DIST_JAVA_OPTS="-Xms256M -Xmx700M -XX:MaxMetaspaceSize=256M -XX:MaxDirectMemorySize=128M -XX:+UseG1GC"

pkill -f launcher.jar 2>/dev/null || true
pkill -f paper-26.3 2>/dev/null || true
sleep 1

rm -f /tmp/server.in /tmp/server.log
mkfifo /tmp/server.in
cd "$D"
# reader first (tail blocks on open until the writer side opens), then
# hold the write end open for the lifetime of the server
tail -f /tmp/server.in 2>/tmp/tail.err | "$JAVA" -Xmx2G -jar launcher.jar > /tmp/server.log 2>&1 &
SRV=$!
exec 9>/tmp/server.in
echo "launcher pid $SRV"

for i in $(seq 1 150); do
  if grep -q "pack finalized (ONE-TIME, static)" /tmp/server.log 2>/dev/null; then break; fi
  if ! kill -0 $SRV 2>/dev/null; then echo "SERVER DIED"; tail -40 /tmp/server.log; exit 1; fi
  sleep 2
done
grep -q "pack finalized (ONE-TIME, static)" /tmp/server.log || { echo "TIMEOUT waiting for finalize"; tail -40 /tmp/server.log; exit 1; }
sleep 2
echo "=== gates ==="
python3 /home/z/my-project/c-LoD/scripts/e2e/gates.py /tmp/server.log
echo "=== server is up (stdin: /tmp/server.in, log: /tmp/server.log) ==="
