# Benchmarks

`make bench` runs `bench/throughput.sh`: a 100 MB mixed ASCII + SGR payload fed (a) straight to
the emulator and (b) through `cat` on a real PTY, written to `bench/results/<date>.json`.
It measures the PTY → parser → grid path headlessly; GPU frame time is not included.

`bench/vtebench.sh` runs [vtebench](https://github.com/alacritty/vtebench), which must run
inside the terminal under test. Compare against Alacritty on the same machine (SPEC §13: no
test slower than 2x Alacritty; `cat` of 100 MB within 1.5x).

`bench/gif.sh` (also run by `make bench`) exports a scripted 60 s 80×24 shell session to GIF at
10 fps and writes the time, size and frame count to `bench/results/<date>-gif.txt`. Budget
(SPEC §10.10.6): under 10 s on an M1 and under 5 MB.
