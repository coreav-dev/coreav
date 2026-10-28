# Gating run of bundle 4, eight-thread arms and identity

Bars B4-1 to B4-6 of `docs/design/h264.md`, "Bundle 4: a banded picture's lines and its aprons' rows owned ahead", "Bars". `base` = `main` at `1e7dd54b`; `k` = `perf/h264-bundle4` code commit `e348cacd`; null arm `base`; seed 20261030.
Built here alike (`cargo build --locked --profile dist --example decode -p coreav-h264`, `RUSTFLAGS` unset); the sha256 of both binaries equal those of the request (`binaries.sha256`). Harness: `scripts/h264_parity_gcp.py`, `h264_parity.py` of `main` (`tools.txt`). FFmpeg 9.0.2 (BtbN `n9.0.2-17-g2a571b6068`). The same binaries ran both runs.

- `gcp-train/`: 8 threads, `c4-standard-16` (us-central1-a, one thread a core, Xeon 8581C), 32 training streams, ten rounds, `--min-seconds 3`. Not void: reference drift +0.30%, 0 unresolved, largest null deviation 0.61%. 2.69 h, spend bound 3.23 USD.
- `gcp-heldout/`: the same on the 64 held-out streams, five rounds. Not void: drift +0.27%, 0 unresolved, largest null deviation 0.70%. 2.80 h, 3.37 USD. Total GCP spend bound 6.59 USD.
- `identity-and-host/identity.txt`: `k` identical to `base` on 32 of 32 training streams at 1 and 8 threads (B4-6's conformance half is the branch's).
- B4-4 (one thread) is not in this directory: on this VM every host-gate check failed for over an hour and a half (FFmpeg's gate streams at 1.4 to 1.9 times the 2.90 GHz reference, erratic from check to check, so not a level a REBASE could take; `identity-and-host/hostgate.log.first-start`, `wait-gate.log`). No 1T call ran here (`STOPPED`). B4-4 runs on another VM of the same class (`1t/`).

Median of the per-stream ratio `k / base`, 95% interval over streams:

| bar | measure | result | bar | |
|---|---|---|---|---|
| B4-1 | 8T discard, 32 | **0.9832** [0.9752, 0.9894], 30/32 faster | <= 0.990, upper < 1.000 | met |
| B4-2 | 8T discard, sd | **0.9788** [0.9573, 0.9869] | <= 0.990 | met |
| B4-3 | 8T discard 720p / 1080p / 4k; write over 32 | 0.9843 / 0.9956 / 0.9856; write 0.9789 [0.9728, 0.9810] | 720p <= 0.995; 1080p, 4k <= 1.005; write <= 1.000 | met |
| B4-5 | 8T discard, held-out 64 | **0.9869** [0.9784, 0.9910], 56/64 faster (sd 0.9848, 720p 0.9767, 1080p 0.9832, 4k 0.9957) | <= 1.000 | met |
| B4-6 | output | identity 32/32 here | | met |

8T write by class (training): sd 0.9793, 720p 0.9732, 1080p 0.9879, 4k 0.9779; held-out write 0.9799.

Against FFmpeg 9.0.2 (information, median per-stream ratio, below 1 is faster than FFmpeg), 8 threads: training discard `base` 0.849 -> `k` 0.836 (32 of 32 faster both), write 0.865 -> 0.849; held-out discard 0.848 -> 0.827 (61 -> 62 of 64), write 0.850 -> 0.834.
