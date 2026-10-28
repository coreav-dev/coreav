# Inter placement round, IP-2 (eight threads, the 32 training streams, base against p): Compute Engine, not void

`c4-standard-16`, `threadsPerCore=1`, `us-central1-a`, STANDARD, Xeon Platinum 8581C @ 2.30 GHz; harness of main `2074409e`, `--threads 8 --min-seconds 3 --rounds 10 --shuffle-seed 20261026 --null-arm base --gate-stream 36501d448eaa.0.720p.h264 --gate-threads 8`,
base = main `acf3ee2f`, p = `b6b5de86` (two copies of base's binary are timed as the null arm). `verdict.json`: not void, reference drift +2.1%, no unresolved stream. Spend bound 3.152 USD (2.627 h at 1.20 USD an hour). Analysis: `../analysis-8t/` (the one-thread arms were not run).
