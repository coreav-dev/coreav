# Gating run of the CAVLC slice loop round (bars SL-1 to SL-6), one-thread calls

`base` = `9ba67d12` (an earlier change's code commit), `k` = `3fc1ee31` (`perf/h264-cavlc-slice-loop`). Helper VM 2 (Intel Xeon @ 2.90 GHz, family 6 model 106; calibration
`helper2-calibration/` on `measured/h264-helper2-calibration`); `provenance.txt`: binaries' sha256 (`k`, and `k` built without `fast`), harness (`scripts/h264_parity.py` of
`main` at `ae609547`), FFmpeg 9.0.2, stream checks. `run-1t.sh` (`NOFAST=1`, `SEED=20261027`): `--null-arm base --gate-stream 36501d448eaa.0.720p.h264 --gate-every 4`, one thread
pinned to core 2, 20 rounds on the 32 training streams (g), 5 rounds on the 64 held-out streams (h), host gate before and after each call (`hostgate.log`).

**Output (SL-6, this run):** `k` and `k` without `fast` are byte-identical to `base` on the 32 training streams at 1 and 8 threads (`identity.txt`, 32 of 32). Conformance is the branch's own evidence.

**Host faults recorded.** Four void calls (kept as `*.void<N>`; each rerun passed both gates): `h.720p` attempts 1 and 2 (after-gate 720p +9.0%, 1080p +5.4%), `h.4k` attempts 1 and 2.
The run script's own `timeout` (5 h, set by me) ended the process during the first `h.4k` attempt (`h.4k.log.killed-by-timeout`); that call alone was run again with the same binaries,
harness and options by `resume-class.sh` (attempts 1 to 3 there; the third passed). The held-out calls were noisy: 5 streams of `h.1080p` and 6 of `h.4k` stayed unresolved after their retries
(kept in the medians), 0 in `h.sd` and `h.720p`; in the training calls one stream, `65ec61f95a00` (720p). The eight-thread guard (SL-4) was not run, since SL-2, SL-3 and SL-5 did not pass.

## Result (`analysis.txt`, `analyze-sl.py`): SL-1 met; SL-2, SL-3 and SL-5 not met

- **SL-1** the six CAVLC streams, one thread, discard: 0.9802 [0.9462, 0.9984], 5 of 6 faster (at most 0.990, upper end below 1.000): **met**. Write 0.9798.
- **SL-2** all 32, one thread: discard 0.9987 [0.9961, 1.0041] (at most 1.000: met); write **1.0010** [0.9952, 1.0031] (at most 1.000): **not met**.
- **SL-3** class medians, one thread: discard sd 0.9900, 720p 1.0022, 1080p 0.9961, 4k **1.0055** (at most 1.005): **not met**; write 0.9939, 1.0005, 1.0003, 1.0010 (met).
- **SL-4** not run (see above).
- **SL-5** one thread, discard, the 64 held-out streams: **1.0020** [0.9979, 1.0053], 27 of 64 faster (at most 1.000): **not met**; write 0.9985.
- Information: `6f7799768413` discard 0.9716, write 0.9738; the 15 held-out CAVLC streams (`pps_entropy.py`) discard 0.9680 [0.9551, 0.9710], 13 of 15 faster; write 0.9775.

Against FFmpeg 9.0.2 (median per-stream ratio, discard, one thread): training `base` 1.038 (14 of 32 faster), `k` 1.032 (14 of 32); the six CAVLC streams 0.969 then 0.950;
`6f7799768413` 1.137 then 1.104; held-out `base` 1.027 (25 of 64), `k` 1.031 (23 of 64).
