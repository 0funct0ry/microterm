#!/usr/bin/env bash
# 100 MB throughput benchmark (SPEC §13): parser-only and `cat` through a PTY.
# Writes bench/results/<date>.json. Usage: bench/throughput.sh [size-mb]
set -euo pipefail
cd "$(dirname "$0")/.."
size_mb="${1:-100}"
out="bench/results/$(date +%Y-%m-%d).json"
cargo run --release -q -p mt_term --example throughput -- --size-mb "$size_mb" --out "$out"
