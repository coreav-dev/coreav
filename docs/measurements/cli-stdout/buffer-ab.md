# 8 KiB against 64 KiB into a pipe (development evidence, not a bar)

`coreav remux IN -` into `cat >/dev/null`, timed with `bench.py`'s
`run_once` (blocking waits). Eleven rounds, with the three builds' order
rotated in each round; each cell is the median. No host-health gate. The
run was on the 2.90 GHz VM class on 2026-10-23, right after run-04, with
the inputs of run-04 on the RAM disk.

- base: `28b2b22` (origin/main), as in run-04.
- 8 KiB: `media_stdout()` with 8 KiB into a pipe (`PIPE_BUFFER = 1 << 13`).
- 64 KiB: the run-04 build (64 KiB into a pipe or a socket).

Both variants were built from an uncommitted tree that differed only in that
constant. The 8 KiB build writes as the shipped code does; the shipped code
also drops the pipe check. The 64 KiB design was rejected
(docs/design/cli-stdout.md, "A larger buffer").

| Input | base (ms) | 8 KiB (ms) | 64 KiB (ms) | 64 KiB / 8 KiB |
|---|---|---|---|---|
| 108521a89a73 | 4.93 | 5.04 | 4.63 | 0.920 |
| a648a2693846 | 11.16 | 10.24 | 9.10 | 0.889 |
| bcf9f3749940 | 22.53 | 20.75 | 18.44 | 0.889 |
| 44cc2b274a94 | 28.57 | 28.04 | 30.05 | 1.072 |
