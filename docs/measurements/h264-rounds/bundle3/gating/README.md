# Gating run of bundle 3

Bars B3-1 to B3-8 of `docs/measurements/h264-rounds/bundle3/plan.md`, "Bars". `base` = `main` at `974f46c1`; `k` = branch `perf/h264-bundle3` at `e40e6db2` (its `crates/` are `72cf01fe`'s); null arm `base`; seed 20261027.
Both decoders built once, alike (`cargo build --locked --profile dist --example decode -p coreav-h264`, `RUSTFLAGS` unset; `binaries.sha256`); the same two binaries ran every arm. Harness: `scripts/h264_parity.py` and `h264_parity_gcp.py` of `main` at `46545e08` (`tools.txt`). FFmpeg 9.0.2 (BtbN `n9.0.2-17-g2a571b6068`).

- `1t/`: one thread pinned to core 2, `--min-seconds 3`, in-call gate `36501d448eaa.0.720p`, a VM of the 2.90 GHz class (family 6, model 106; calibration in `helper3-vm/calibration/` of branch `measured/h264-helper3-calibration`), every call between two host gates (`hostgate.log`: no after-check failed, no call void).
  The 32 training streams at twenty rounds (`g.*`), the 64 held-out streams at five (`h.*`). `identity.txt`: `k` identical to `base` on 96 of 96 streams (32 training, 64 held-out) at 1 and 8 threads.
- `gcp-train/`: eight threads on `c4-standard-16` (us-central1-a, one thread a core, Xeon 8581C), 32 training streams, ten rounds, `--min-seconds 3`. Not void: reference drift +1.8%, 0 unresolved, largest null deviation 0.63%. 2.64 h, spend bound **3.17 USD**.
- Interruption of the one-thread run, recorded: the script was started with a 10 h `timeout` and was killed by it during the held-out 1080p call (`1t/h.1080p.log.killed`, no report; nothing of it was read). Calls done by then: `g.sd` to `g.4k`, `h.sd`, `h.720p`.
  `run-b3-1t-rest.sh` ran the remaining two held-out calls with the same binaries, harness and options. Its first start found the host steadily 5% faster than the class reference on the 1080p gate stream and stopped before a call (`1t/STOPPED.first-rest-start`);
  the gate's level-shift rule (REBASE, `1t/REBASE.md`, committed before the restart) moved the reference to the median of six steady checks, and the restart ran both calls under it (`h.1080p`, `h.4k`: attempt 1 each, gates passed). Paired ratios do not depend on the gate's level.
- `analyze-b3.py` -> `analysis.txt`, `summary.json`.

Median of the per-stream ratio `k / base`, 95% interval over streams:

| bar | measure | result | bar | |
|---|---|---|---|---|
| B3-1 | 1T discard, 32 | **0.9888** [0.9820, 0.9908], 29/32 faster | <= 0.990, upper < 1.000 | met |
| B3-2 | 1T discard, the 16 streams at 1080p and 4k | **0.9887** [0.9787, 0.9911], 15/16 faster (1080p 0.9895, 4k 0.9853) | <= 0.985 | **missed by 0.0037** |
| B3-3 | 1T write, 32 | 0.9873 [0.9827, 0.9915] | <= 1.000 | met |
| B3-4 | 1T each class, discard / write | sd 0.9921 / 0.9899, 720p 0.9853 / 0.9870, 1080p 0.9895 / 0.9896, 4k 0.9853 / 0.9775 | <= 1.005 | met |
| B3-5 | 8T, 32, discard / write | 0.9982 [0.9961, 1.0005] / 0.9988 [0.9977, 1.0012] | <= 1.005 each | met |
| B3-6 | 8T each class, discard / write | sd 0.9977 / 0.9986, 720p 0.9995 / 0.9981, 1080p 0.9954 / 0.9969, 4k 1.0033 / 1.0056 | <= 1.010 | met |
| B3-7 | 1T discard, held-out 64 | 0.9884 [0.9855, 0.9896], 60/64 faster (sd 0.9897, 720p 0.9866, 1080p 0.9829, 4k 0.9866); write 0.9871 | <= 1.000 | met |
| B3-8 | output | identity 96/96 here; the rest is the branch's `checks.md` | | met |

B3-2 is missed on valid calls (no host fault, gates passed; its calls' largest null deviation 2.2%, one stream unresolved each at 1080p and 4k, flagged and kept); by the bundle's rule "the bundle ships if it meets B3-1 to B3-8" it does not ship as composed.
Against FFmpeg 9.0.2 (information, median per-stream ratio, below 1 is faster than FFmpeg): 1T discard, training, `base` 1.031 -> `k` 1.009 (15 -> 16 of 32 faster), write 0.992 -> 0.973; held-out discard 1.015 -> 1.003 (27 -> 30 of 64), write 0.996 -> 0.980; 8T discard, training, 0.872 -> 0.870, write 0.900 -> 0.898 (31 and 29 of 32 faster).
