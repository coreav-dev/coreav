# The packed mvd caches (`perf/h264-mvd-packed`), one-thread arms

`base` = main `46545e08`, `k` = `a3f55038` (identity: 32 of 32 streams byte-identical at 1 and 8 threads, `identity.txt`).
Host: `Intel(R) Xeon(R) Processor @ 2.90GHz`, family 6 model 106, 8 CPUs; calibration of this VM: `h264-rounds/helper4-vm/calibration` on branch `measured/h264-helper4-calibration` (`aa07172d`).
Script: `run-1t-two-arms.sh mvd-packed 46545e08 a3f55038` of `measured/h264-helper-setup` (`c7519b9f`); harness `scripts/h264_parity.py` of main `46545e08`; FFmpeg 9.0.2 (BtbN `n9.0.2-17-g2a571b6068`).
Calls: `g` (32 training, one thread pinned to core 2, 20 rounds, `--null-arm base`, in-call gate) and `h` (64 held-out, 5 rounds), per class, between two host gates (`hostgate.log`: 16 PASS; two before-checks failed (`h.720p` 1.080, `h.1080p` 1.144 of reference), were retried a minute apart and passed; no call voided).
`analysis-1t.txt` is the output of `analyze-generic.py . mv`. Eight-thread arms (MV-4) are run on Compute Engine by helper 1 (not here).

| bar | value | |
|---|---|---|
| MV-1 g discard all 32, at most 0.995, upper < 1.000 | 0.9867 [0.9819, 0.9881] | pass |
| MV-2 g write all 32, at most 1.000 | 0.9815 [0.9774, 0.9900] | pass |
| MV-3 g each class, discard / write, at most 1.005 | worst 0.9885 (4k discard) / 0.9867 (720p write) | pass |
| MV-5 h discard all 64, at most 1.000 | 0.9851 [0.9818, 0.9894] | pass |

Streams faster: g discard 30/32, write 31/32; h discard 51/64, write 50/64. Unresolved after two retries (kept in the medians): g `ec473c99c538.0.sd`, `f9e973f44570.0.sd`, `4b54de28c2b3.0.4k`; h six 1080p streams and `b716b0c80a63.0.4k` (listed in `analysis-1t.txt`).
Home paths and the account name are redacted (`$HOME`, `$USER`). Spend: none (local VM).
