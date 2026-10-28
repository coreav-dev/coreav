# Deblocking round: the kernels' setup in the form the tables give

`docs/design/h264.md`, "Deblocking round: the kernels' setup in the form the tables give", states the probe that
motivated the round (`../deblock-setup-probe/`), the design and bars DS-1 to DS-5, fixed before any timing.

- `base` = `main` at `70a4cec`; `k` = `e03cf436`, the branch's code commit. Before the rebase onto `70a4cec` (earlier changes) they were `b366429` and `a65f2dc`; the local A/Bs and the probe below ran on those, `fad4826` being
  `a65f2dc` without the out-of-line per-edge paths.
- `run-ab-helper.sh BASE K` is the gating run, `analyze.py DIR` prints the bars from its reports; the run's output goes
  to `gating/`. The harness runs with `--shuffle-seed 20261026 --null-arm base --gate-stream
  41e324ebb708.0.sd.h264 --gate-every 4`, and `--min-seconds 3` on the eight-thread calls ("Measurement method: host faults inside a
  call").
- Instructions on the first pictures (callgrind, AVX2 kernels), `k` over `base`: median 0.9787, 32 of 32 fewer; sd
  0.9880, 720p 0.9697, 1080p 0.9817, 4k 0.9798. Rebased (`70a4cec` against `e03cf436`): 0.9752, 32 of 32 fewer; sd
  0.9842, 720p 0.9687, 1080p 0.9803, 4k 0.9769.
- Local timing A/B of `fad4826` (information): discard 0.9932 [0.9899, 1.0017], 21 of 32 faster; 4k 1.0087, from a
  shift of the CABAC loops' alignment (design note).
- Local timing A/B of `a65f2dc` before gating (information, not a bar; one thread pinned, six rounds, the 32 streams):
  discard 0.9923 [0.9892, 0.9964], 23 of 32 faster (sd 0.9872, 720p 0.9885, 1080p 0.9946, 4k 1.0072); write 0.9905
  [0.9885, 0.9969], 24 of 32.
- Local timing A/B of the rebased commits (`70a4cec` against `e03cf436`; information, one thread pinned, six rounds,
  the 32 streams): discard 0.9857 [0.9821, 0.9894], 30 of 32 faster (sd 0.9820, 720p 0.9843, 1080p 0.9850, 4k
  0.9959); write 0.9849 [0.9825, 0.9919], 26 of 32.
- `conformance.txt`: 97 of 97 at 1, 2 and 8 threads, detected and scalar kernels, on `e03cf436`. `fuzz.txt`: `filter_edge`, `filter_chroma_pair` and `filter_chroma_mb`,
  600 s each, clean (on `a65f2dc`, whose kernels crate the rebase left unchanged); `luma_mb_edges` (the window's edges against edges checked one at a time), 600 s, clean.
- `gating/`: the gating run (`run-ab-helper.sh 70a4cec e03cf436` on the helper's VM, family 6 model 106; evidence
  branch `measured/h264-deblock-setup-gating`, `40a10fb7`; its own `README.md` lists every call in order, the
  aborted eight-thread calls included). DS-1 to DS-5 are met: one thread, discard 0.9848 [0.9803, 0.9935], 27 of 32
  faster; write 0.9857 [0.9810, 0.9925]; per class discard sd 0.9805, 720p 0.9782, 1080p 0.9943, 4k 0.9945; eight
  threads, discard 0.9836 [0.9724, 0.9951], write 0.9900 [0.9851, 0.9954]. Against FFmpeg 9.0.2 at one thread,
  discard 1.097 (`base`) and 1.085 (`k`); at eight threads 1.009 and 0.969. The design note's "Result: passed".
