# bS kernel round, eight-thread arms on Compute Engine

`run-gcp.sh bb7255cd 193051ba OUT` of the branch, with two changes recorded in `run-gcp.helper3.sh`: `--max-hours 4.1` instead of 2.5 (the cap stays 5 USD: 4.1 x 1.20 = 4.92; earlier runs by other helpers timed out at 3 h) and the provenance written outside the (must-be-empty) `--out` directory, then copied in.
One `c4-standard-16` (us-central1-a, STANDARD, threads per core 1, Intel Xeon Platinum 8581C, family 6 model 207; `host.txt`), ten rounds, `--min-seconds 3`, seed 20261026, null arm `base`; `verdict.json`: not void
(reference drift -2.4%, 0 unresolved streams in all four classes). 2.66 hours, spend bound **3.20 USD**. Results in `../gating/README.md`.
