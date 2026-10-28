# Entropy round: DecodeDecision in assembly: gating runs

Arms: `base` = `ac315f8c` (the code of an earlier change, `fded8560`), `k` = `200f9a79` (`perf/h264-cabac-decision-asm`), and the two builds without the feature `fast` (`basenofast`, `knofast`, information). Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1), `gating/binaries.sha256`
(base `3cd2a75f...`, k `149e5042...`; the two `--no-default-features` builds are byte-identical, `f107bbc9...`, since the assembly is compiled only with `fast`). Bars AD-1 to AD-6 were committed before the build. Harness: `scripts/h264_parity.py` of main `241a7c77`, FFmpeg 9.0.2 `0d709a0b...`,
`--shuffle-seed 20261026 --null-arm base --gate-stream 36501d448eaa.0.720p.h264 --gate-every 4`. `gating/analyze.py` and `gating/run-1t.sh` are the scripts used (written for the helper's one-thread calls; the eight-thread arms are on Compute Engine).

- `gating/`: one thread on the helper's VM (Intel Xeon @ 2.90 GHz, family 6 model 106): `g` (32 training streams, pinned `--pin 2 --rounds 20`, base, k, basenofast, knofast) and `h` (64 held-out streams, pinned, 5 rounds). Identity 32 of 32 (`k` and `knofast` at 1 and 8 threads against `base` at 1). Host gate:
  17 passes, 1 failed check (waited, passed), no void. The run was **killed by a restart of the machine** during `g.1080p` (a host fault, recorded: the call's partial log is `g.1080p.log.killed-by-restart`), and resumed from `g.1080p` with the same binaries (`RESUME=1`); `g.sd` and `g.720p` had finished and are kept.
  The unresolved streams of every call: none. Spend: none.
- `gating-gcp/`: eight threads on Compute Engine (`c4-standard-16`, one thread a core, `us-central1-a`, Xeon Platinum 8581C), the 32 training streams, 10 rounds, `--min-seconds 3`, `scripts/h264_parity_gcp.py` of `241a7c77`: `verdict.json` not void (reference drift -0.6%), no stream unresolved. Spend bound 3.226 USD.
  `gating-gcp-void1/` is the first run, void: it ran out of its 3 h limit during the 4k call (job status 124; the sd call had 4 streams unresolved from the gate baseline defect fixed in an earlier change). Spend bound 3.507 USD. Total for the round's Compute Engine runs: **6.733 USD**.

Result (`gating/analysis.txt`, with `gating-gcp/t8.*.json` read as `t8`): `AD-1 pass, AD-2 pass, AD-3 pass, AD-4 pass, AD-5 pass` (AD-6, output, is read from `identity.txt` and the conformance reports). `k` over `base`, median per-stream ratio (95% bootstrap interval), faster of the streams:

| Call | discard, all | write, all | per class, discard (sd, 720p, 1080p, 4k) |
|---|---|---|---|
| `g`, 1 thread, 32 training | 0.9700 [0.9645, 0.9742] (31/32) | 0.9686 [0.9644, 0.9748] (31/32) | 0.9848, 0.9693, 0.9560, 0.9682 |
| `t8`, 8 threads, Compute Engine | 0.9756 [0.9670, 0.9802] (29/32) | 0.9778 [0.9702, 0.9855] (30/32) | 0.9909, 0.9746, 0.9652, 0.9744 |
| `h`, 1 thread, 64 held-out | 0.9719 [0.9655, 0.9790] (58/64) | 0.9705 [0.9684, 0.9781] (55/64) | 0.9789, 0.9647, 0.9785, 0.9663 |

Against FFmpeg 9.0.2 (median per-stream ratio, base then k; share where ours is faster): `g` discard 1.095 (9/32) then 1.041 (12/32), write 1.045 (12/32), 1.016 (14/32); `t8` (Compute Engine) discard 0.939 (25/32), 0.906 (28/32), write 0.928 (23/32), 0.909 (26/32);
`h` discard 1.074 (18/64), 1.039 (23/64), write 1.044 (23/64), 1.014 (28/64). `knofast` over `basenofast` at one thread (the builds are identical): discard 0.9997 [0.9987, 1.0020], write 0.9993; the safe build against FFmpeg: discard 1.118 (7/32). The eight-thread numbers are of the Compute Engine host and are not compared with the helper VM's.
