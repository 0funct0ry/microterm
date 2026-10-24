#!/usr/bin/env bash
# GIF export benchmark (SPEC §10.10.6): a 60 s 80x24 shell session at 10 fps.
# Budget: under 10 s on an M1 and under 5 MB. Writes bench/results/<date>-gif.txt.
set -euo pipefail
cd "$(dirname "$0")/.."
out="bench/results/$(date +%Y-%m-%d)-gif.txt"
mkdir -p bench/results
cargo test --release -q -p mt_export gif_bench -- --ignored --nocapture 2>&1 \
  | grep '^gif_bench:' | tee "$out"
