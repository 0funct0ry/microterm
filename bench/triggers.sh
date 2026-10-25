#!/usr/bin/env bash
# Trigger benchmarks (SPEC §10.11.10):
#   * the worker scans at least 200 000 lines/s with 8 rules (`mt_triggers` worker_bench);
#   * `cat` of 100 MB through a PTY with the line tap and 8 rules is within 10 % of the run
#     without (the `pty_cat_triggers.relative` field of throughput.sh's JSON).
# Writes bench/results/<date>-triggers.txt. Usage: bench/triggers.sh [size-mb]
set -euo pipefail
cd "$(dirname "$0")/.."
size_mb="${1:-100}"
out="bench/results/$(date +%Y-%m-%d)-triggers.txt"
mkdir -p bench/results
{
  cargo test --release -q -p mt_triggers worker_bench -- --ignored --nocapture 2>&1 | grep '^triggers_bench:'
  cargo run --release -q -p mt_term --example throughput -- --size-mb "$size_mb" --triggers 8 2>/dev/null \
    | python3 -c '
import json, sys
d = json.load(sys.stdin)
t = d["pty_cat_triggers"]
print("triggers_cat: size_mb=%d off=%.1f MB/s on=%.1f MB/s relative=%.3f lines=%d hits=%d" % (
    d["size_mb"], d["pty_cat"]["mb_per_s"], t["mb_per_s"], t["relative"], t["lines_scanned"], t["hits"]))
if t["relative"] < 0.90:
    print("triggers_cat: FAIL (more than 10 % slower than with triggers off)")
    sys.exit(1)
'
} | tee "$out"
