# The predict round's 4k class, local check (information, not a bar)

Before the gating request, as the note says: the 8 4k training streams, 150 frames, the four builds of the bars
(`base` 678bd69, `w` 4754464, `k1` fe97dea, `k2` 7140ae8; `binaries.sha256`, `cargo build --locked --profile dist
--example decode -p coreav-h264` each), `scripts/h264_parity.py --pin 2 --rounds 5 --require-cpu "@ 2.90GHz"` on the
2.90 GHz Intel host (family 6, model 106), between two host gates (`hostgate.log`: PASS 0.999/0.986 before, 0.980/0.961
after). Median per-stream ratio, 95% bootstrap interval (2,000 resamples, `random.Random(20261023)`):

| Pair | discard | write |
|---|---|---|
| k2/base | 0.9541 [0.8557, 0.9808], 7/8 faster | 0.9538 [0.9103, 0.9864], 7/8 |
| w/base | 1.0037 [0.9237, 1.0224], 3/8 | 1.0062 [0.9553, 1.0180], 3/8 |
| k1/w | 1.0353 [1.0170, 1.0518], 0/8 | 1.0270 [1.0153, 1.0471], 0/8 |
| k2/k1 | 0.9163 [0.8956, 0.9400], 8/8 | 0.9262 [0.8938, 0.9601], 8/8 |

Round 14's call site alone (`w`) reads about 0.4% slower on 4k here, the direction of its rejected second run (1.7%);
`k2` gains over it. Output: the 32 training streams identical to `base` with `w`, `k1` and `k2` at one thread, short
lengths.
