# AV1 process-boundary reference figures

These are reference figures, not a judged verdict or a performance claim. On three lossy H.264 inputs, sandboxed CoreAV took 1.063 times FFmpeg's end-to-end time (geometric mean of per-input ratios), and in-process CoreAV took 1.037 times. The encoder process boundary accounts for 39% of the summed time gap. The rest is outside the boundary and has the same size as the decode-path difference seen in separate decode-only calls.

All three arms produced byte-identical AV1 packet streams for each input: one packet hash per input across all nine calls, the same payload bytes and the same PSNR. So the timing differences come from the pipeline around the encoder, not from different encoder work.

The method is in [docs/design/av1-process-reference.md](../../../docs/design/av1-process-reference.md). Per-call records are in [calls.json](calls.json). [summary.json](summary.json) is the output of `python3 scripts/av1_process_reference.py summarize bench/encoders/av1-process-reference/calls.json`, and replaying that command reproduces it byte for byte.

## Host and builds

- One on-demand Compute Engine c4-standard-8 VM with simultaneous multithreading off: Intel Xeon Platinum 8581C (Emerald Rapids), 4 visible CPUs, one thread per core, 31.5 GB memory, Debian 13, Linux 6.12. Nothing else ran on it. Details are in [host.json](host.json).
- CoreAV: the `av1-measurement` profile build of the `cli,av1-measurement` features for `x86_64-unknown-linux-musl`, SHA-256 `c678981e410bf4ad41b0ce264a748cc2dfdaaa381d5281812a96b70dfd338800`.
- FFmpeg comparator: the oracle revision, built as a static musl binary with the same SVT-AV1 4.2.0 source by `scripts/build_av1_reference_ffmpeg.sh`, SHA-256 `2021edb7212c646859fc733b0a25310c99e437f3f85fb3e277318daf58018824`. Every call reported SVT-AV1 v4.2.0, one session, a GOP of 126 frames and parallelism level 1.
- Metrics and probes: the pinned oracle FFmpeg 9.0.2 from `scripts/oracle-ffmpeg.env`.

## End-to-end calls

Median of three calls per arm with rotated order. S is sandboxed CoreAV, I is in-process CoreAV, F is FFmpeg. Times are wall-clock seconds from process start to successful exit.

| Input | Geometry | Frames | S | I | F | S/F | I/F | S-I | I-F | Boundary share |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| input01 | 854x480, 25 fps | 751 | 10.063 | 9.877 | 9.542 | 1.055 | 1.035 | 0.186 | 0.335 | 0.36 |
| input02 | 1280x720, 23.976 fps | 720 | 12.994 | 12.466 | 11.870 | 1.095 | 1.050 | 0.528 | 0.596 | 0.47 |
| input03 | 1920x1080, 29.97 fps | 901 | 66.858 | 65.924 | 64.279 | 1.040 | 1.026 | 0.935 | 1.645 | 0.36 |
| Sum of medians | | | 89.916 | 88.267 | 85.691 | | | 1.649 | 2.576 | 0.39 |

The geometric means of the per-input ratios are 1.063 (S/F) and 1.037 (I/F). Every arm's three-call range was narrower than its total gap, so none of the shares is marked unstable. Removing the boundary alone would cut the summed CoreAV time by 1.8% (S/I 1.019; 1.014 to 1.042 per input).

The median CPU accounting shows where the extra time is spent. Across inputs, the sandbox adds 0.3 to 1.4 s of system time over in-process CoreAV, with almost no change in user time. In-process CoreAV uses 0.2 to 0.8 s more user time and 0.3 to 2.0 s more system time than FFmpeg. At 1080p, user time (68 s) is close to wall time (64 to 67 s): the single-session encoder at parallelism level 1 limits all three arms.

| Input | S user / system | I user / system | F user / system |
|---|---:|---:|---:|
| input01 | 11.11 / 0.91 | 11.09 / 0.61 | 10.85 / 0.29 |
| input02 | 13.87 / 1.89 | 13.74 / 1.40 | 13.27 / 0.62 |
| input03 | 68.48 / 4.82 | 68.31 / 3.43 | 67.49 / 1.44 |

## Decode-only diagnostic

Each implementation decoded each input to the same complete 8-bit 4:2:0 raw stream (identical byte counts and SHA-256), then made three alternating calls writing to the null device. CoreAV used its native decoder's default host-core workers, and FFmpeg's decoder was given the same 4 workers. These calls include demuxing and raw output, so they are a decode-path reference, not a decoder-kernel benchmark.

| Input | CoreAV seconds | FFmpeg seconds | Ratio | Difference |
|---|---:|---:|---:|---:|
| input01 | 0.969 | 0.705 | 1.37 | 0.264 |
| input02 | 1.393 | 0.929 | 1.50 | 0.464 |
| input03 | 3.667 | 1.909 | 1.92 | 1.758 |

The summed decode-path difference (2.49 s) is close to the summed residual I-F (2.58 s). The end-to-end calls decode and encode at the same time, so this agreement is consistent with the residual being mostly decode-side, but it does not prove it. The time CoreAV spends inside its decoder calls during transcoding (`decode` in calls.json) overlaps encoding and is not added to either figure. CoreAV's decode-only calls also used more system time than FFmpeg's (2.45 s against 0.82 s at 1080p).

## What this means for encoder isolation

Running SVT-AV1 in-process would remove about 1.8% of end-to-end CoreAV time on these inputs and about two fifths of the gap to FFmpeg. In exchange, it would give up crash isolation, the separate address-space limit and the ability to stop a stuck encoder. The larger part of the gap is outside the boundary. These figures are input for a future isolation proposal; they change no policy, and production builds keep the sandbox.

## Limits

- Three lossy inputs, each a 30-second video-only stream-copy cut of existing test media; no lossless set.
- One VM and three calls per arm. The ranges are small, but this is not a confidence interval.
- The measurement build runs one continuous encoder session per input, so the sandboxed arm does not include production session segmentation.
