# Gating run of the banded-hints round

Bars HB-1 to HB-6 of `docs/design/h264.md`, "Threading round: a banded picture's lines asked for ahead".

- `base` = `main` at `641fb937` (the run was made before an earlier change merged; `main`'s decoder code is that of `7af5d4f5`, the branch's merge base).
- `k` = `perf/h264-8t-hints` (head `6bb8ab1a`) rebased on `base`: `d728aba9` (a clean rebase; `crates/coreav-h264` and `crates/coreav-kernels-h264` identical to the branch's; the branch's head is kept as `rejected/h264-banded-hints-gated`, and the same change rebased on a later `main` as `rejected/h264-banded-hints`). Built once, alike (`cargo build --locked --profile dist --example decode -p coreav-h264`,
  `RUSTFLAGS` unset; `binaries.sha256`); the same two binaries ran every arm. Harness: `scripts/h264_parity.py`, `h264_parity_gcp.py` of `main` at `641fb937` (`tools.txt`). FFmpeg 9.0.2 (BtbN `n9.0.2-17-g2a571b6068`). Null arm `base`, seed 20261028.
- `gcp-train/`: eight threads on `c4-standard-16` (us-central1-a, one thread a core, Xeon 8581C), 32 training streams, ten rounds, `--min-seconds 3`. Not void: reference drift +0.02%, 0 unresolved, largest null deviation 0.63%. 2.66 h, spend bound 3.20 USD.
- `gcp-heldout/`: the same on the 64 held-out streams, five rounds. Not void: drift +0.36%, 0 unresolved, largest null deviation 1.27%. 2.70 h, 3.23 USD. Total GCP spend bound **6.43 USD**. (A first start of both runs in the same second made two VMs of one name; both were refused at creation, 0.10 USD of bound, and are not counted as runs.)
- `1t/`: one thread pinned to core 2, six rounds, the 32 training streams, discard and write, on a VM of the 2.90 GHz class (family 6, model 106; calibration in `helper3-vm/calibration/` of branch `measured/h264-helper3-calibration`), host gate before and after each call (`hostgate.log`: no check failed after a call, no void). `run-hb-1t.sh` is the one-thread script of the earlier rounds with these arms.
  `1t/identity.txt`: `k` identical to `base` on 32 of 32 streams at 1 and 8 threads (HB-6's conformance half is the branch's, `../checks.txt`; `analysis.txt`'s `conformance.txt` is that file).
- `analyze-hb.py` -> `analysis.txt`, `summary.json`.

Median of the per-stream ratio `k / base`, 95% interval over streams:

| bar | measure | result | bar | |
|---|---|---|---|---|
| HB-1 | 8T discard, 32 | **0.9884** [0.9787, 0.9963], 26/32 faster | <= 0.996, upper < 1.000 | met |
| HB-2 | 8T discard, sd | **0.9952** [0.9801, 1.0001] | <= 0.995 | **missed by 0.0002** |
| HB-3 | 8T discard, 720p / 1080p / 4k; write over 32 | 0.9796 / 0.9932 / 0.9883; write 0.9824 [0.9690, 0.9915] | <= 1.005 each; <= 1.000 | met |
| HB-4 | 1T pinned, discard / write, 32 | 0.9992 [0.9974, 1.0032] / 0.9990 [0.9940, 1.0066] | <= 1.003 each | met |
| HB-5 | 8T discard, held-out 64 | 0.9893 [0.9777, 0.9950], 48/64 faster (sd 0.9886, 720p 0.9769, 1080p 0.9852, 4k 1.0004) | <= 1.000 | met |
| HB-6 | output | identity 32/32 at 1 and 8 threads (this run); conformance the branch's | | met |

HB-2 is missed by 0.0002 on a run that is not void (no host fault); by the round's rule one run decides and a narrow miss is not a reason to run again. Other numbers, 8T write by class: sd 0.9921, 720p 0.9645, 1080p 0.9818, 4k 0.9767.
Against FFmpeg 9.0.2 (information, median of the per-stream ratio, a ratio below 1 is faster than FFmpeg): 8T discard, training, `base` 0.875 -> `k` 0.866 (31 of 32 faster than FFmpeg both), write 0.899 -> 0.876; held-out discard 0.871 -> 0.852 (59 -> 61 of 64);
1T discard, training, 1.022 -> 1.016 (15 of 32 faster both), write 0.990 -> 0.981.
