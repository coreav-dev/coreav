# Deblocking round: thresholds by average QP, and only live edges

`docs/design/h264.md`, "Deblocking round: thresholds by average QP, and only live edges", states the hypothesis, the
design and bars DD-1 to DD-5, fixed before any timing of `k`. The round was rejected by DD-4 and its code is not in
`main`: `k.diff` is the change measured (`git diff fb893fd e2bc974` over the crates).

- `base` = `main` at `fb893fd`; `k` = `e2bc974`, the branch's code commit.
- `instructions.txt`: first-picture instructions of `base` and `k` (callgrind, one thread, discard; sd 90 pictures,
  720p 30, 1080p 12, 4k 4), information, not a bar.
- `run-ab-helper.sh BASE K` is the gating run, `analyze.py DIR` prints the bars from its reports; the run's output goes
  to `gating/`.

## Result

Rejected by DD-4. `gating/` (one run of `run-ab-helper.sh fb893fd e2bc974` on the bar class host, Intel Xeon @
2.90 GHz, family 6 model 106; branch `measured/h264-deblock-driver-gating` `e26e8a0`): `k` over `base`, one thread,
discard 0.9948 [0.9895, 0.9989], 22 of 32 faster, write 0.9944 (DD-1 to DD-3 met); eight threads, discard 1.0088
[0.9979, 1.0181] (DD-4 missed: above 1.005), 4k 1.0197 with none of 8 faster; output identical on 32 of 32.
Against FFmpeg 9.0.2, one thread discard: `base` 1.145, `k` 1.139. The design note's "Result" section has the
table.
