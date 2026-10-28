# Probe PW: a band's publishing woken twice, at eight threads

Plan and rule, committed before the run: `plan-probe.md`. Probe code: branch `probe/r38-publish-wake` (`4368781d`),
`base` = `main` at `13e4dfc4`; both byte-identical on the 32 training streams at 1 and 8 threads
(`identity-probe.txt`: `../view-path/ident.sh` of `rejected/h264-view-path`, which compares every stream's
output at 1 and 8 threads with `base`'s at 1 and prints only the streams that differ; `binaries.sha256`).

## Run

`pw-run1/`: `scripts/h264_parity_gcp.py`, `c4-standard-16` in `us-central1-a`, one thread a core, 32 training
streams, 5 rounds, seed 20261027, the harness of `main` at `241a7c77` (an earlier change's in-call gate) with the `--threads`
option of `docs/h264-view-path-1t` left at its default of 8. Not void (`pw-run1/verdict.json`): the reference moved
+0.38% between the first call and the last, no stream unresolved. Null arm: median per stream −0.03% (discard) and
+0.06% (write), at most 1.1% either side (`pw-run1/null.txt`, `null.py`). Spend: VM 1.34 h, bound 1.61 USD,
and a start before an earlier change's gate, stopped before any call reported, 0.17 h, bound 0.20 USD:
**1.81 USD** in all.

## Result (`pw-run1/pw-summary.txt`, `pw.py`)

| `pw2` / `base`, eight threads | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| discard | **1.0015** [1.0000, 1.0029]; `pw2` faster on 10 of 32 | 1.0053 | 0.9995 | 1.0028 | 0.9999 |
| write | 1.0006 [0.9995, 1.0025] | 1.0039 | 0.9993 | 0.9979 | 1.0007 |

**PW-1 is not met** (at least 1.015 over the 32 or in a class): **no round is built.** A second lock and wake on every
band published costs 0.15% of the eight-thread wall time, 0.5% at sd, where a band is one macroblock row. The
change in view would remove at most about that: the wake where nobody waits is not where an earlier change's 6.0% for the
bands' publishing and waits goes on this host. Of that 6.0%, the wake on publishing was 2.41; the rest is the
waits themselves (`../scaling-8t/README.md`, cause 3), which this change leaves. Why the wake's cycles cost so little
wall time was not measured.

The change itself (a band published with no reader waiting skips the lock and the wake; a count of waiters with
sequentially consistent fences on both sides, and a stress test) is kept on `rejected/h264-publish-wake` as a
candidate for a bundle round. It is byte-identical to `base` on the 32 streams at 1 and 8 threads (fast and safe
builds), and passes conformance (97 of 97, detect and scalar kernels, fast and safe, at 1, 2 and 8 threads) and its
stress test (`fix-checks/`).

Against FFmpeg 9.0.2 (information): `base` 0.9338 [0.8728, 0.9669] at eight threads, 25 of 32 faster (sd 0.9761,
720p 0.9416, 1080p 0.9771, 4k 0.7497).
