#!/usr/bin/env bash
# CRUSSTY module build: produces liblod.so (see module.json "main").
set -euo pipefail
cd "$(dirname "$0")"
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release
echo "liblod.so -> $(pwd)/target/release/liblod.so"
