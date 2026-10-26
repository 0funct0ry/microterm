# Benchmarks against the SPEC §13 targets

Measured with `bench/throughput.sh` (100 MB mixed ASCII and SGR, release build, headless). GPU frame
time, cold start, memory and the Alacritty comparison need the GUI and a second terminal on the same
machine; they are part of the manual pass in `docs/release-checklist.md`.

| Date | Machine | Parser | `cat` via PTY | `cat` via PTY, 8 trigger rules |
|---|---|---|---|---|
| 2026-10-26 | Apple Silicon Mac (macOS), debug-free release build | 106 MB/s | 76 MB/s | 64 MB/s (0.84 of untriggered) |

Against the targets:

| Target | Result |
|---|---|
| `cat` 100 MB within 1.5× of Alacritty | not measured here (needs Alacritty on the same machine) |
| `cat` 100 MB with 8 trigger rules within 10 % | **0.84 on this single run: over the 10 % budget.** The trigger bench (`bench/triggers.sh`) is the authoritative check; re-run both before tagging |
| `vtebench`, cold start, keypress latency, idle memory and CPU | manual, per release |

Raw JSON for each run is in `bench/results/` (git-ignored).
