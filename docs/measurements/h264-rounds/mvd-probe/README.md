# CABAC mvd sites: probe and build

`plan.txt` holds the question, the cut in view, the rules and the results, each part written before the run it
governs (the plan at `20af3a71`, the probe code at `31cab434`, the probe result at `92ed635c`, all on
`probe/h264-mvd-pad`; the build `6f504762` on `perf/h264-mvd-packed`). Base is `perf/h264-cabac-decision-asm`
`4b909fd8` (code `200f9a79`: change A's residual dispatch and change B's DecodeDecision in assembly), the code
`entropy-after-483` profiled.

| file | what |
|---|---|
| `sites.txt`, `calls.py` | per stream, calls and self instructions of the three sites on base (`calls.py` over `entropy-after-483`'s callgrind records) |
| `counts.txt` | per CABAC stream: the calls and the predicted cut |
| `probe-pad.diff`, `verify.txt` | the probe's pads (feature `probe-pad`) and its check: instructions added against the prediction, output md5 |
| `ab.txt`, `ab-summary.txt` | the probe's timing (`ab3.py`, 8 rounds; `abq.py` for the medians and intervals) |
| `c-count.txt` | C's instructions over base on the 32 streams |
| `c-identity.txt`, `c-conformance.txt` | C's output against base (32 streams, 1 and 8 threads) and conformance |
| `c-ab.txt`, `c-ab-summary.txt` | C's timing |
| `binaries.sha256` | the three timed binaries (`dist`, one toolchain, `RUSTFLAGS` unset) |

Host: a development VM, Intel Xeon @ 2.90 GHz, family 6; one thread pinned to CPU 3, CPU time. Directional; the gating
bars are in `docs/design/h264.md` ("Entropy round: the CABAC motion vector differences"). Spend: 0 USD.

`abq.py` reads the CABAC/CAVLC split from a local file of the entropy measurement (`/tmp/ent/bins.pkl`, the streams
with significance-map bins); the six CAVLC streams are `5185721dda11`, `6f7799768413`, `9db41aa16b5a`, `b71b6b31e54f`,
`ec473c99c538` and `f9e973f44570`.
