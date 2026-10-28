# CAVLC residual block parse: probe and build

`plan.txt` holds the question, the cut in view, the rules and both results, each part written before the run it
governs (the plan at `c64f7fba`, the probe code at `57b27723`, the probe result at `fe04e23d`, all on
`probe/h264-cavlc-block-pad`; the build `97514aa6` on `rejected/h264-cavlc-block`). Base is `perf/h264-bundle2-a`
`a46be406`: `main` `16c3e239`'s decoder with bundle 2 item A (the CAVLC level reading), which the cut builds on.

| file | what |
|---|---|
| `counts.txt` | per CAVLC stream: block_body calls, coded blocks, total_zeros and run_before reads, levels; its instructions |
| `probe-pad.diff`, `verify.txt` | the probe's pads (feature `probe-pad`) and its check: instructions added against the prediction, output md5 |
| `ab.txt`, `ab-summary.txt` | the probe's timing (`ab3.py`, 8 rounds; `abq.py` for the medians and intervals) |
| `c-count.txt` | C's instructions over base on the six CAVLC streams |
| `c-identity.txt`, `c-conformance.txt` | C's output against base (32 streams, 1 and 8 threads) and conformance |
| `c-ab.txt`, `c-ab-summary.txt` | C's timing |
| `binaries.sha256` | the three timed binaries (`dist`, one toolchain, `RUSTFLAGS` unset) |

Host: a development VM, Intel Xeon @ 2.90 GHz, family 6; one thread pinned to CPU 3, CPU time. Directional; no gating
bar was run. Spend: 0 USD.

`abq.py` reads the CABAC/CAVLC split from a local file of the entropy measurement (`/tmp/ent/bins.pkl`, the streams
with significance-map bins); the six CAVLC streams are `5185721dda11`, `6f7799768413`, `9db41aa16b5a`, `b71b6b31e54f`,
`ec473c99c538` and `f9e973f44570`.
