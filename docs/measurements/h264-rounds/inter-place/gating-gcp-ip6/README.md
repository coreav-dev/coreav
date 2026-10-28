# Inter placement round, IP-6 (eight threads, the 64 held-out streams, base against p): the second Compute Engine run, not void

`c4-standard-16`, `threadsPerCore=1`, `us-central1-a`, STANDARD, Xeon Platinum 8581C @ 2.30 GHz; harness of main `241a7c77` (an earlier change: the gate baseline of agreeing samples, a drift confirmed), `--threads 8 --min-seconds 3 --rounds 5 --shuffle-seed 20261026 --null-arm base
--gate-stream 36501d448eaa.0.720p.h264 --gate-threads 8`, base = main `acf3ee2f`, p = `b6b5de86`. `verdict.json`: not void, reference drift -0.05%, no unresolved stream. Spend bound 3.182 USD (2.652 h at 1.20 USD an hour).
The first run of IP-6 (`gating-gcp-ip6-void1/` on branch `measured/h264-inter-place-gating`) was void.
