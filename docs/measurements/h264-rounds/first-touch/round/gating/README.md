# Gating run of the first-touch round

`base` = `main` at `538265c4`, `ft` = `68c8f059` (named `k` in the harness files), null arm = `base`; bars FT-1 to FT-7 of `docs/design/h264.md`, "Threading round: a new picture's first touch on the thread that decodes it".
Both decoders were built once, alike (`cargo build --locked --profile dist --example decode -p coreav-h264`, `RUSTFLAGS` unset; `binaries.sha256`), by `gcp_round.sh` (the builds ran pinned to cores 4-7, nice 19); the same two binaries ran all three arms.
Harness: `scripts/h264_parity.py` and `h264_parity_gcp.py` of `main` at `a7819e9` (`gcp-train/tools.txt`). FFmpeg 9.0.2 (BtbN `n9.0.2-17-g2a571b6068`).

- `gcp-train/`: eight threads, `c4-standard-16` (us-central1-a, one thread a core, Xeon 8581C), the 32 training streams, ten rounds, `--min-seconds 3`, seed 20261027, null arm `base`, in-call gate `36501d448eaa.0.720p`. Not void (reference drift -0.1%, 0 unresolved). 2.62 h, spend bound 3.14 USD.
- `gcp-heldout/`: the same on the 64 held-out streams (`docs/measurements/h264-n2-heldout/`, 64 of 64 matching its `es.json`), five rounds. Not void (drift +0.7%, 0 unresolved). 2.70 h, 3.24 USD. Total GCP spend bound **6.38 USD** (cap 10).
- `1t/`: one thread pinned to core 2 on a VM of the 2.90 GHz class (family 6, model 106; calibration in `helper3-vm/calibration/` of branch `measured/h264-helper3-calibration`), twenty rounds, seed 20261027, null arm `base`, in-call gate `36501d448eaa.0.720p`, every call between two host gates
  (`hostgate.log`: all eight checks PASS, no void). `run-ft-1t.sh` is the one-thread script of the earlier rounds with these arms. `identity.txt`: `ft` identical to `base` on 32 of 32 streams at 1 and 8 threads (conformance is the branch's).
- `analyze-ft.py` -> `analysis.txt`, `summary.json`.

| median of ft/base | all 32 | sd | 720p | 1080p | 4k | bar |
|---|---|---|---|---|---|---|
| 8T discard (training) | **0.9890** [0.9847, 0.9935] | 0.9934 | 0.9842 | 0.9937 | **0.9803** | FT-1 <= 0.995 and upper < 1.000; FT-2 4k <= 0.985; FT-4 each <= 1.005 |
| 8T write (training) | 0.9884 [0.9841, 0.9928] | 0.9913 | 0.9832 | 0.9932 | 0.9637 | FT-3 <= 1.000; FT-4 |
| 1T discard (training) | 0.9967 [0.9934, 0.9981] | 0.9980 | 0.9991 | 0.9949 | 0.9917 | FT-5 all <= 1.005, each class <= 1.010 |
| 1T write | 0.9952 [0.9935, 0.9982] | 0.9978 | 0.9973 | 0.9950 | 0.9905 | FT-5 |
| 8T discard (held-out 64) | **0.9880** [0.9851, 0.9916], 55/64 faster | 0.9944 | 0.9874 | 0.9870 | 0.9784 | FT-7 <= 1.000 |

FT-1 to FT-5 and FT-7 pass; FT-6 is the branch's (identity re-checked here). Information, the ratio to FFmpeg 9.0.2 (median of the per-stream ratio): training 8T discard `base` 0.936 -> `ft` 0.919 (26 of 32 faster than FFmpeg); held-out 8T discard 0.952 -> 0.936 (46 of 64); 1T discard 1.081 for `ft`.

Flagged, not a bar: the one-thread 4k call ended with 5 unresolved streams (the harness's host-fault records; its host gates before and after passed, and the gate drift marks were the in-call gate reading 7% below its baseline). The call is kept as run; the 4k class read 0.9917 against a bar of 1.010, and the other three calls had none unresolved.
