# Kernel round: deblocking thresholds built in vector registers

`docs/design/h264.md`, "Kernel round: deblocking thresholds built in vector registers", states the hypothesis, the
design and bars TL-1 to TL-5, fixed before the gating run.

- `base` = `main` at `baa6d0a`; `k` = `d7e7fb0`, the branch's code commit.
- `run-ab-helper.sh BASE K` is the gating run, `analyze.py DIR` prints the bars from its reports; the run's output goes
  to `gating/`.
- Local timing A/B before gating (information, not a bar): discard median `k` over `base` 0.9932 [0.9911, 0.9984], 24
  of 32 faster; write 0.9959 [0.9924, 0.9995].
- `conformance.txt`: 97 of 97 at 1, 2 and 8 threads, detected and scalar kernels, on `k`.

## Result

Passed: TL-1 to TL-5 are met. `gating/` holds one run of `run-ab-helper.sh baa6d0a d7e7fb0` on the bar class host (Intel
Xeon @ 2.90 GHz, family 6 model 106), from branch `measured/h264-deblock-lanes-gating` `5953e04`. `k` over `base`:
- one thread, discard 0.9954 [0.9923, 0.9985], 23 of 32 faster; write 0.9960 [0.9919, 0.9987];
- every class at most 1.0023;
- eight threads, discard 0.9986, write 0.9903;
- output identical on 32 of 32.

Against FFmpeg 9.0.2, one thread, discard: `base` 1.136, `k` 1.136; write 1.088 → 1.083. The design note's "Result"
section has the tables.
