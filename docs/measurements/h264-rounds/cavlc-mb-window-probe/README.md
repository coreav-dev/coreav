# CAVLC macroblock layer: one bit window across header and residual, probe and build

`plan.txt` holds the question, the cut in view, the rules and the results, each part written before the run it
governs (the plan at `89b7bc76`, the probe code at `03a5be14`, the probe result at `f3d608e4`, all on
`probe/h264-cavlc-mb-window-pad`; the build `51acb19f` on `rejected/h264-cavlc-mb-window`). Base is
`perf/h264-cavlc-window` `9ba67d12` (`main` `ae609547` with the CAVLC window round), whose residual window the cut
carries across the macroblock.

| file | what |
|---|---|
| `counts.txt`, `sites.txt`, `sites.py`, `calls.py` | per CAVLC stream at the class frame counts: `ue` calls, their self instructions, the calls by site, the predicted cut |
| `probe-pad.diff`, `verify.txt`, `verify1.sh` | the probe's pads (feature `probe-pad`) and its check: instructions added, output md5 |
| `ab.txt`, `ab-summary.txt` | the probe's timing (`ab3.py`, 8 rounds; `abq.py` for the medians and intervals) |
| `c.diff` | C, the change as built |
| `c-count.txt` | C's instructions over base |
| `c-identity.txt`, `c-conformance.txt`, `ident1.sh` | C's output against base (32 streams, 1 and 8 threads) and conformance |
| `c-ab.txt`, `c-ab-summary.txt` | C's timing |
| `binaries.sha256` | the three timed binaries (`dist`, one toolchain, `RUSTFLAGS` unset) |

Host: a development VM, Intel Xeon @ 2.90 GHz, family 6; one thread pinned to CPU 3, CPU time. Directional. Spend: 0 USD.

`abq.py` reads the CABAC/CAVLC split from a local file of the entropy measurement (`/tmp/ent/bins.pkl`); the six CAVLC
streams are `5185721dda11`, `6f7799768413`, `9db41aa16b5a`, `b71b6b31e54f`, `ec473c99c538` and `f9e973f44570`.
