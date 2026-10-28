# CABAC motion vector differences, MV-4: the eight-thread guard on Compute Engine

Arms: `base` = main `46545e08`, `k` = `a3f55038` (`perf/h264-mvd-packed`), built on a build host with `dist`, `--locked` (sha256 base `2d4f7ee8...`, k `9ec0ae04...`). Harness of main `46545e08` (`scripts/h264_parity_gcp.py`, an earlier change's gate): `c4-standard-16`, `threadsPerCore=1`, `us-central1-a`, STANDARD,
Xeon Platinum 8581C @ 2.30 GHz; 32 training streams, `--threads 8 --min-seconds 3 --rounds 10 --shuffle-seed 20261026 --null-arm base --gate-threads 8`. `verdict.json`: not void (reference drift -0.5%, no stream unresolved). Run only after the one-thread bars passed (`measured/h264-mvd-packed-gating-1t`).
Spend bound 3.18 USD (2.65 h at 1.20 USD an hour).

Result (`analysis.txt`): `MV-4 pass` (a guard: at most 1.005 over all 32 and at most 1.010 per class). `k` over `base` at eight threads, discard: overall 0.9964 [0.9948, 0.9983] (27/32 faster); sd 0.9985, 720p 0.9983, 1080p 0.9944, 4k 0.9952; the 26 CABAC streams 0.9954; write overall 0.9980 [0.9945, 0.9996] (24/32).
Largest null deviation 0.6%. Against FFmpeg 9.0.2 on this host (median per-stream ratio, base then k; share where ours is faster): discard 0.852 (32/32) then 0.854 (32/32), write 0.869 (30/32), 0.866 (30/32); per class discard (k) sd 0.887, 720p 0.858, 1080p 0.890, 4k 0.629.
