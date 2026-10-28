# CAVLC block calls: one window across a macroblock's residual, probe and build

`plan.txt` holds the question, the cut in view, the rules and the results, each part written before the run it
governs (the plan at `bc9c01e9`, the probe code at `bf5a033d`, the probe result at `4ea8109a`, all on
`probe/h264-cavlc-window-pad`; the build `c3625731` on `perf/h264-cavlc-window`). Base is `perf/h264-bundle2`
`c5557ada` (bundle 2, items A to J), whose CAVLC block parse (items A and J) the cut works around.

| file | what |
|---|---|
| `counts.txt`, `cnt1.sh`, `calls.py` | per CAVLC stream at the class frame counts: block calls, their self instructions, the predicted cut |
| `probe-pad.diff`, `verify.txt`, `verify1.sh` | the probe's pads (feature `probe-pad`) and its check: instructions added against the prediction, output md5 |
| `ab.txt`, `ab-summary.txt` | the probe's timing (`ab3.py`, 8 rounds; `abq.py` for the medians and intervals) |
| `c-count.txt` | C's instructions over base |
| `c-identity.txt`, `c-conformance.txt`, `ident1.sh` | C's output against base (32 streams, 1 and 8 threads) and conformance |
| `c-ab.txt`, `c-ab-summary.txt` | C's timing |
| `binaries.sha256` | the three timed binaries (`dist`, one toolchain, `RUSTFLAGS` unset) |

Host: a development VM, Intel Xeon @ 2.90 GHz, family 6; one thread pinned to CPU 3, CPU time. Directional; the gating
bars are in `docs/design/h264.md` ("CAVLC round: one bit window across a macroblock's residual"). Spend: 0 USD.

`abq.py` reads the CABAC/CAVLC split from a local file of the entropy measurement (`/tmp/ent/bins.pkl`); the six CAVLC
streams are `5185721dda11`, `6f7799768413`, `9db41aa16b5a`, `b71b6b31e54f`, `ec473c99c538` and `f9e973f44570`.
