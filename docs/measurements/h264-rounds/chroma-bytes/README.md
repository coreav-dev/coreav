# Chroma deblocking on bytes: where the 16-bit kernel's cycles went, and the kernel on bytes

The evidence of `docs/design/h264.md`, "Kernel round: chroma deblocking on bytes". Diagnosis and local timing only;
nothing here is a gated speed claim.

## Diagnosis of the 16-bit kernel (`main` at `d95bb8b1`)

- **Cycles per instruction by part** (`cpi_parts.py`, `cpi-parts.txt`): an earlier change's cycle and instruction samples at each
  address of the kernel (`raw.tar.gz` on `measured/h264-profile-acf3ee2f-pmu`, the rebuild of `acf3ee2f` that
  `../kernel-gaps/` uses), by the part the address's inline frames name. 0.430 over the kernel; the vertical pass
  0.31 (its transposes) to 0.45 (its filter), the horizontal pass 0.47 to 0.48. No part stands out.
- **Store forwarding.** The horizontal pass reads eight bytes from column 2 of window rows 0 to 7, of which rows 2 to
  7 the vertical pass has just written as eight bytes from column 0: the reads overlap the writes only in part, so
  they wait for the writes to reach the cache. A timing-only build reading from column 0 instead (wrong output, same
  work) against `main`, pinned alternating pairs on the 2.90 GHz class: `41e324ebb708` 1.0025 (15 pairs, 900
  pictures), `6f7799768413` 1.0014 (11 pairs). Not the gap.
- **Branch misses**: the kernel's functions are below the twelve largest of the deblocking stage's branch misses in
  an earlier change's `run1/stages.txt` (`../pmu-profile/`), each under 2% of the stage's.
- **Ports.** `llvm-mca` (LLVM 19) of the kernel's straight-line code as compiled (branches removed): with
  `-mcpu=sapphirerapids`, 187 cycles an iteration with port 5 at 185; with `-mcpu=icelake-server`, 159 with port 5
  at 157. Measured on this class (`ports.c`: eight independent instructions repeated, the best of 200 runs, TSC
  ticks at 2.90 GHz an instruction; `vpaddb` 0.30 is three a cycle): `vpunpcklbw`, `vpunpcklqdq`, `vpshufb`,
  `vpshufd` 0.42 (two a cycle, ports 1 and 5); `vpackuswb`, `vinserti128` and `vextracti128` from a register 0.83
  (one a cycle, port 5); `vpavgb` 0.42; `vpblendd` 0.34; `vpblendvb` 1.19. The 16-bit kernel's fully filtered
  macroblock, from its disassembly: 16 `vinserti128`, 14 `vpmovzxbw`, 12 `vpbroadcastw`, 12 `vmovd`, 8 `vextracti128`
  and 8 `vpextrq` (port-5-only), and about 80 in-lane shuffles.

## The kernel on bytes

`cmb.rs` is the microbenchmark (an example of `coreav-kernels-h264` when copied into its `examples/`, not
committed there): one row of 60 macroblocks of both planes, Cb's and Cr's buffers 1,344 bytes apart modulo 4 KiB,
called seven macroblocks apart, smooth samples with all strengths set (`all`), only the vertical (`v`) or only the
horizontal edges (`h`), or none (`off`, the copy and the call); ns a macroblock, the best of 30 runs of 3,000 rows,
pinned to core 2; instructions from callgrind (100 rows, `off` subtracted).

| | `main` ns | bytes ns | `main` instructions | bytes instructions |
|---|---|---|---|---|
| `off` | 11.5 | 11.5 | | |
| `v` | 40.8 | 32.8 | 278 | 196 |
| `h` | 32.5 | 25.3 | 226 | 142 |
| `all` | 63.0 | 47.9 | 483 | 302 |

On a stream (`41e324ebb708`, 300 pictures, callgrind): `filter_chroma_mb` runs 23.48 million instructions on `main`
and 19.11 million on bytes, over 58,487 calls; 50,305 calls run the vertical pass and 50,942 the horizontal one.

Counted in a throwaway build on four training streams (`41e324ebb708`, `9db41aa16b5a`, `1d4b1775d78a`,
`3b1b441e23ae`, 300 pictures): passes in which no line passes the thresholds are 30 of 100,000, 189 of 660,000, 35 of
1,380,000 and 2,553 of 140,000; passes with a bS of 4 or more on some line are 52%, 92%, 36% and 40%. So the kernel
neither leaves a pass early nor branches around the strong filter.

## Local timing

`scripts/h264_parity.py --decoder base=<main d95bb8b1> --decoder k=<e4cabc07> --frames <class count> --pin 2 --rounds
6 --shuffle-seed 7`, per class on the training set, this VM (2.90 GHz class), the median per-stream ratio `k` over
`base` with a 95% bootstrap interval:

```
k/base discard n=32 median 0.9996 [0.9853,1.0052] faster 17/32  sd 1.0025 720p 0.9888 1080p 1.0005 4k 0.9991
k/base write n=32 median 0.9984 [0.9862,1.0032] faster 17/32  sd 1.0002 720p 1.0022 1080p 1.0001 4k 0.9600
```

The interval spans 1.000: under 1% to gain, below what this host shows alone. The change waits for a bundle round
(`rejected/h264-chroma-deblock-bytes`).

## Checks on the code commit

- Unit tests: the lanes against their definition on parameters of any bytes; the filter on bytes against the
  clause's arithmetic on every p0 and q0 against extremes and a grid of p1 and q1 at t'C0 0, 25 and 31, and on 3.2
  million random lines; the macroblock against the reference on table parameters (8,000 cases) and on parameters of
  any bytes (3,000).
- `identity.txt`: the 32 training streams byte-identical to `main` at one thread (detected kernels and AVX2 alone)
  and at eight.
- `conformance.txt`: 97 of 97 at 1, 2 and 8 threads, detected kernels and scalar.
- `fuzz.txt`: `filter_chroma_mb`, 600 s, 24.6 million runs, no failure.
