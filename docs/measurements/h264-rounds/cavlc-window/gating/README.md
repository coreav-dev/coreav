# Gating run of the CAVLC window round (bars CW-1 to CW-6)

`base` = `main` at `ae609547` (bundle 2 merged), `k` = `9ba67d12` (`perf/h264-cavlc-window`). One-thread calls on helper VM 2 (Intel Xeon @ 2.90 GHz, family 6 model 106;
calibration `helper2-calibration/` on `measured/h264-helper2-calibration`); `provenance.txt` has the binaries' sha256 (`k` and `k` built without `fast`), the harness
(`scripts/h264_parity.py` of `main` at `ae609547`), FFmpeg 9.0.2 and the stream checks. `run-1t.sh` (`NOFAST=1`): `--shuffle-seed 20261026 --null-arm base
--gate-stream 36501d448eaa.0.720p.h264 --gate-every 4`, one thread pinned to core 2, 20 rounds on the 32 training streams (g), 5 rounds on the 64 held-out streams (h),
host gate before and after each call (`hostgate.log`).

**Output (CW-6, this run):** `k` and `k` built without `fast` are byte-identical to `base` on the 32 training streams at 1 and 8 threads (`identity.txt`, 32 of 32).
The conformance runs are the branch's own (`cavlc-window-probe/after-main.txt`) and were not repeated here.

**Void calls:** one, `g.720p` attempt 1 (after-gate: 1080p stream 4.630 s, +5.5%); files `*.void1`, the second attempt passed both gates.
**Unresolved after retries (kept in the medians):** g `ae16bb7c7192` (4k); h `178de4a3a5a2`, `334602b1b17e`.

**Eight threads (CW-4):** `gcp/`, `c4-standard-16`, `us-east4-a`, one thread a core (Xeon Platinum 8581C), STANDARD, `--threads 8 --min-seconds 3`, ten rounds, `--null-arm base`, tools of
`main` at `46545e08` (`gcp/tools.txt`), started by `gcp8-retry.sh`. Not void (`gcp/verdict.json`: reference drift +1.6%, no unresolved stream; largest null deviation of a
stream 1.2% against the 2% bar). Spend 3.129 USD (upper bound of the hours the VM existed).

## Result (`analysis-final.txt`, `analyze-cw.py`): CW-1 to CW-5 met

- **CW-1** the six CAVLC streams, one thread, discard: 0.9848 [0.9834, 0.9946], 6 of 6 faster (at most 0.990, upper end below 1.000); write 0.9889.
- **CW-2** all 32, one thread: discard 0.9962 [0.9925, 0.9969], 29 of 32 faster; write 0.9935 [0.9924, 0.9956], 28 of 32 (at most 1.000 each).
- **CW-3** class medians, one thread: discard 0.9938, 0.9964, 0.9973, 0.9931 (sd, 720p, 1080p, 4k); write 0.9932, 0.9935, 0.9974, 0.9919 (at most 1.005 each).
- **CW-4** eight threads, discard, all 32: 1.0024 [0.9999, 1.0034] (at most 1.005); classes 0.9993, 1.0021, 1.0036, 1.0033 (at most 1.010 each). Write, information: 1.0014.
- **CW-5** one thread, discard, the 64 held-out streams: 0.9919 [0.9893, 0.9961], 47 of 64 faster (at most 1.000).
- Information: `6f7799768413` discard 0.9835, write 0.9849; the held-out CAVLC streams (15 of 64; `pps_entropy.py` reads entropy_coding_mode_flag of each stream's PPS; it
  names the training six exactly): discard 0.9921 [0.9832, 0.9983], 11 of 15 faster; write 0.9814.

Against FFmpeg 9.0.2 (median per-stream ratio, discard): one thread, training `base` 1.042 (13 of 32 faster), `k` 1.040 (14 of 32); the six CAVLC streams 0.978 (3 of 6) then 0.967 (4 of 6);
`6f7799768413` 1.158 then 1.139; held-out `base` 1.037 (25 of 64), `k` 1.033 (24 of 64); eight threads on Compute Engine `base` 0.882 (30 of 32), `k` 0.879 (29 of 32).
