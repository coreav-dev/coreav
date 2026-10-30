# Gating run of the CAVLC slice function round (bars SF-1 to SF-7)

`base` = `main` at `3f918b38`, `k` = `983e8bc3` (`perf/h264-cavlc-slice-fn`; the branch's later commits change a comment in `decoder.rs` and docs only). One-thread calls on helper VM 2
(Intel Xeon @ 2.90 GHz, family 6 model 106; calibration `helper2-calibration/` on `measured/h264-helper2-calibration`); `provenance.txt`: binaries' sha256 (`k`, and `k` built without `fast`), the
harness (`scripts/h264_parity.py` of `main` at the start), FFmpeg 9.0.2, stream checks. `run-1t.sh` (`NOFAST=1`): `--shuffle-seed 20261026 --null-arm base --gate-stream 36501d448eaa.0.720p.h264
--gate-every 4`, one thread pinned to core 2, 20 rounds on the 32 training streams (g), 5 rounds on the 64 held-out streams (h), host gate before and after each call (`hostgate.log`).

**Output (SF-7, this run):** `k` and `k` built without `fast` byte-identical to `base` on the 32 training streams at 1 and 8 threads (`identity.txt`, 32 of 32). Conformance is the branch's own evidence.

**Host faults recorded.** The host was noisy for hours (the gate's FFmpeg 5 to 50% over its reference with nothing else running here). Four calls were void, each kept as `*.void<N>` and run again with the same
binaries: `g.1080p` (02:52 and 05:29), `g.4k` (08:52), `h.1080p` (10:32; the copies `*.keep-*` hold a void file that a later attempt's name would have overwritten, `*.void0-first-run` the first). When the gate
would not pass for ten minutes before `g.1080p`, `run-1t.sh` stopped (`host noisy`); `resume-rest.sh` and `resume-class.sh` (included) then ran the remaining calls with the same binaries, harness and options,
waiting five minutes between tries. Unresolved after their retries, kept in the medians: seven training streams (`7650ae466748`, `f9e973f44570`, `76e0b1692494`, `9db41aa16b5a`, `6da34958296c`, `a81de82106be`,
`b71b6b31e54f`), no held-out stream.

**Eight threads (SF-6):** `gcp/`, `c4-standard-16`, `us-east4-a`, one thread a core (Xeon Platinum 8581C), STANDARD, `--threads 8 --min-seconds 3`, ten rounds, `--null-arm base`, tools of `main` at `bb0281d7`
(`gcp/tools.txt`), `gcp8-retry.sh`. Not void (`gcp/verdict.json`: reference drift -0.9%, no unresolved stream; largest null deviation 0.84% against the 2% bar). Started once SF-1 to SF-5 had passed. Spend 3.173 USD (upper bound).

## Result (`analysis-final.txt`, `analyze-sf.py`): SF-1 to SF-6 met

- **SF-1** six CAVLC streams, one thread, discard: 0.9739 [0.9419, 0.9873], 6 of 6 faster (at most 0.990, upper end below 1.000); write 0.9750; `6f7799768413` 0.9678 / 0.9753.
- **SF-2** all 32, one thread: discard 0.9927 [0.9883, 0.9947], 29 of 32 faster; write 0.9943 [0.9887, 1.0005] (at most 1.003 each).
- **SF-3** class medians, one thread (sd, 720p, 1080p, 4k): discard 0.9841, 0.9986, 0.9902, 0.9921; write 0.9842, 1.0006, 0.9962, 0.9888 (at most 1.005).
- **SF-4** the 26 CABAC streams, one thread: discard 0.9934 [0.9921, 0.9978], 23 of 26 faster; write 0.9966 [0.9919, 1.0026] (at most 1.003 each).
- **SF-5** one thread, discard, the 64 held-out streams: 0.9951 [0.9890, 0.9985], 47 of 64 faster (at most 1.003); write 0.9923.
- **SF-6** eight threads, discard, all 32: 0.9970 [0.9940, 0.9997], 24 of 32 faster (at most 1.005); classes 0.9916, 0.9980, 0.9969, 0.9978 (at most 1.010). CAVLC 6: 0.9829.
- Information: the 15 held-out CAVLC streams (`pps_entropy.py`) discard 0.9686 [0.9637, 0.9772], 15 of 15 faster; write 0.9725.

Against FFmpeg 9.0.2 (median per-stream ratio, discard): one thread, training `base` 0.964 (21 of 32 faster), `k` 0.961 (22 of 32); the six CAVLC streams 0.952 then 0.940; `6f7799768413` 1.089 then 1.054;
held-out `base` 0.957 (42 of 64), `k` 0.953 (44 of 64); eight threads on Compute Engine `base` 0.847 (32 of 32 faster), `k` 0.840 (32 of 32).
