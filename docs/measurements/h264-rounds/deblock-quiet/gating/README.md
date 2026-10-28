# Gating run of the quiet-QP deblocking round (bars QZ-1 to QZ-7), one-thread calls

Helper VM 2 (Intel Xeon @ 2.90 GHz, family 6 model 106, 8 vCPUs; calibration in `helper2-calibration/` on `measured/h264-helper2-calibration`).
`base` = `dd85b381` (the branch's base on `main`), `k` = `4075f864` (the code commit); `provenance.txt` has the binaries' sha256, the harness
(`scripts/h264_parity.py` of `main` at `4f118676`, the run's start), FFmpeg 9.0.2 (BtbN, sha256 `0d709a0b...`) and the stream checks (32 of 32, 64 of 64).
`run-1t.sh` is the script (the method of the residual round's: `--shuffle-seed 20261026 --null-arm base --gate-stream 36501d448eaa.0.720p.h264
--gate-every 4`, one thread pinned to core 2, 20 rounds on the 32 training streams (g), 5 rounds on the 64 held-out streams (h); host gate
before and after each call, `hostgate.log`). Output of `k` identical to `base` on the 32 training streams at 1 and 8 threads (`identity.txt`, 32 of 32).

One call was void: `h.1080p` attempt 1 (after-gate 720p 1.363 s, ratio 1.052; this was while the eight-thread VM's upload ran from this VM); its
files are `*.void1`, the second attempt passed both gates. Unresolved streams after their retries stay in the medians: g `1d4b1775d78a` (720p),
`6da34958296c` (1080p); h `3c331ca6a971` (720p).

`analysis.txt` is `analyze.py`'s output of the branch (bars QZ-1 to QZ-6; QZ-5 is the eight-thread bar and is read from `t8.*`, see below).

## Eight-thread calls (QZ-5), Compute Engine

`gcp/`: `c4-standard-16`, `us-east4-a`, one thread a core (Intel Xeon Platinum 8581C, family 6 model 207, 8 vCPUs), STANDARD provisioning, `--threads 8
--min-seconds 3`, ten rounds, `--null-arm base`, gate stream `36501d448eaa.0.720p`; the tools are those of `main` at `241a7c77` (`gcp/tools.txt`, which has the
in-call gate baseline of an earlier change); `gcp8.sh` and `gcp8-retry.sh` start it (prebuilt arms, the same binaries as the one-thread calls; they retry the VM create
every five minutes while the project's vCPU quota is full: 12 create attempts failed with QUOTA_EXCEEDED, `gcp/create-attempts.txt`). Verdict: not void
(`gcp/verdict.json`: reference before 0.3867 s, after 0.3884 s, drift 0.44%); one stream unresolved after its retries, `c7ecda16e4a7` (1080p), kept in the medians; the
largest null deviation of a stream 1.0% sd, 1.2% 720p, 1.8% 1080p (the unresolved one), 1.4% 4k, against the 2% bar.

Spend: 3.673 USD (3.06 h, upper bound) for the run, about 0.1 USD in the failed create attempts (their bound), 3.7 USD in all.

## Result

`analysis-final.txt` (`analyze.py` of the branch over the one-thread reports and the Compute Engine reports): **QZ-1 to QZ-6 met.**
- QZ-1 one thread, discard, T: 0.9483 [0.9045, 0.9671], 10 of 10 faster (bar at most 0.975, upper end below 1.000).
- QZ-2 write, T: 0.9542 [0.9188, 0.9786] (at most 0.985).
- QZ-3 discard, R: 0.9934 [0.9911, 0.9974] (at most 1.003).
- QZ-4 all 32: discard 0.9909, write 0.9916 (at most 1.000); class medians at most 0.9951 (sd, write; at most 1.005 each).
- QZ-5 eight threads, `c4-standard-16`: all 32 discard 0.9943 [0.9903, 0.9969] (at most 1.000); T 0.9853 (at most 1.000); classes 0.9948, 0.9972, 0.9954, 0.9820 (at most 1.010).
- QZ-6 one thread, 64 held-out streams, discard: 0.9884 [0.9828, 0.9927], 49 of 64 faster (at most 1.000).
- QZ-7 (output): `k` identical to `base` on the 32 training streams at 1 and 8 threads, built with the default features (`identity.txt`, this run); the build without `fast`
  and the conformance runs are the branch's own (`run/identity-nofast.txt`, `run/conformance.txt`) and were not repeated here.

Against FFmpeg 9.0.2 (median of per-stream ratios, information; one thread, discard): training `base` 1.103 (10 of 32 faster), `k` 1.098 (10 of 32); held-out 1.105 (18 of 64), 1.082 (20 of 64).
Eight threads on Compute Engine, discard: `base` 0.950 (25 of 32 faster), `k` 0.947 (24 of 32).
