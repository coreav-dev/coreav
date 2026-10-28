# Gating run of bundle 2 (bars B2-1 to B2-7), one-thread calls

Helper VM 2 (Intel Xeon @ 2.90 GHz, family 6 model 106; calibration `helper2-calibration/` on `measured/h264-helper2-calibration`). `base` = `main` at `bb7255cd`,
`k` = `perf/h264-bundle2` at `c5557ada` (its `crates/` equal `57e7cd8`'s); `provenance.txt` has the binaries' sha256, the harness (`scripts/h264_parity.py` of `main`
`4f118676` at the start), FFmpeg 9.0.2 and the stream checks. `run-1t.sh` is the script: `--shuffle-seed 20261026 --null-arm base --gate-stream
36501d448eaa.0.720p.h264 --gate-every 4`, one thread pinned to core 2, 20 rounds, the 32 training streams, host gate before and after each call (`hostgate.log`; no call void).
Output of `k` identical to `base` on the 32 training streams at 1 and 8 threads (`identity.txt`). `analyze-b2.py` prints the bars from the reports; no stream unresolved.

## Eight-thread calls (B2-2, B2-4, B2-5, B2-7), Compute Engine

Two runs, each `c4-standard-16`, one thread a core (Intel Xeon Platinum 8581C, family 6 model 207, 8 vCPUs), STANDARD provisioning, `--threads 8 --min-seconds 3`,
`--null-arm base`, gate stream `36501d448eaa.0.720p`, tools of `main` at `78afd20d` (training) and `a7819e94` (held-out), `tools.txt`, both with the in-call gate baseline of an earlier change (merged at `241a7c77`), started by `gcp8.sh` and `gcp8-retry.sh`
(prebuilt arms, the one-thread calls' binaries): `gcp-train/` (`us-east4-a`, the 32 training streams, ten rounds) and `gcp-heldout/` (`us-central1-a`, the 64 held-out
streams, five rounds). Neither run void (`verdict.json`): reference drift -2.8% and -0.35%, no stream unresolved; largest null deviation of a stream 1.4% (training)
and 1.35% (held-out), against the 2% bar. The held-out run's first create attempt hit the vCPU quota (`create-attempts.txt`).
Spend (upper bound of the hours the VMs existed): 3.150 USD (3.0 h) and 3.214 USD (2.68 h), 6.36 USD in all.

## Result (`analysis-final.txt`, `analyze-b2.py`): B2-1 to B2-5 and B2-7 met

- **B2-1** one thread, discard, all 32: 0.9818 [0.9770, 0.9853], 32 of 32 faster (at most 0.995, upper end below 1.000).
- **B2-2** eight threads, discard, all 32: 0.9690 [0.9632, 0.9747], 29 of 32 faster (at most 0.990, upper end below 1.000).
- **B2-3** one thread, discard, the six CAVLC streams: 0.9484 [0.9382, 0.9559], 6 of 6 faster (at most 0.985).
- **B2-4** write, all 32: one thread 0.9825 [0.9778, 0.9868], eight threads 0.9707 [0.9672, 0.9772] (at most 1.000 each).
- **B2-5** classes (sd, 720p, 1080p, 4k), medians: one thread discard 0.9666, 0.9811, 0.9849, 0.9839, write 0.9687, 0.9810, 0.9816, 0.9872; eight threads discard
  0.9536, 0.9731, 0.9692, 0.9732, write 0.9529, 0.9739, 0.9734, 0.9763 (at most 1.005 each).
- **B2-7** eight threads, discard, the 64 held-out streams, five rounds: 0.9663 [0.9599, 0.9702], 60 of 64 faster (at most 1.000); write 0.9674; classes 0.9605, 0.9771, 0.9541, 0.9751.
- **B2-6** output: `k` identical to `base` on the 32 training streams at 1 and 8 threads, default build (`identity.txt`, this run); the safe-mode build and the conformance runs are the branch's own evidence (`bundle2/identity-safe.txt`, `bundle2/conformance.txt`) and were not repeated here.

Against FFmpeg 9.0.2 (median of per-stream ratios, information, discard): one thread, `base` 1.090 (10 of 32 faster), `k` 1.059 (14 of 32); eight threads on Compute Engine,
training `base` 0.936 (24 of 32), `k` 0.901 (28 of 32); held-out 64: `base` 0.949 (45 of 64), `k` 0.909 (55 of 64).
