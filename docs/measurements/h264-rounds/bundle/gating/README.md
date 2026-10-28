# Small-bundle round (round 25): gating run on the A/B helper's VM

Arms: `before` = main `3d60f0bf`, `after` = `perf/h264-small-bundle` `ddb4aa24`. Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1, one `CARGO_TARGET_DIR` per arm, no RUSTFLAGS),
`binaries.sha256` (before `f81c8173...`, after `1c7b8f34...`). `run-ab-helper.sh` is the deblock-driver round's script with the arms named `before` and `after` and `g` at ten rounds (the branch has no script of its own);
`analyze.py` of `ddb4aa24` (`python3 -I analyze.py DIR g t8`); FFmpeg 9.0.2 `0d709a0b...`.

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity 32 of 32 (after at 1 and 8 threads against before at 1). Eight calls between gate checks
(`g`: four pinned `--pin 2 --rounds 10`; `t8`: four unpinned `--threads 8 --rounds 10`), `hostgate.log`: 18 passes and 5 failed checks (three before-checks of `g.720p`, each waited and passed; **two voids**:
`g.720p` and `g.4k`, after-checks, each call run again and passed; the voided calls' files are kept as `*.void1`), **REBASE count 0**. A file download and unzip for the next round ran niced on cores 4 to 7
(`taskset -c 4-7`, `nice -n 15`) during the calls, so during `g.1080p` (no void) and `g.4k` attempt 1 (the 64 held-out streams were also extracted that way); the void of `g.4k` attempt 1 (720p reference 1.180 of its reference) falls in that window and may be due to it. It was
stopped before the re-run and ran no more during any timing call. Spend: none (local VM).

Result (`analysis.txt`): `BN-1 False, BN-2 False, BN-3 True, BN-4 False` (BN-5 is read from `identity.txt` and the conformance reports). After over before, one thread pinned, discard: overall 0.9971 [0.9910, 0.9993]
(23/32 faster; bar at most 0.990), write 0.9992 [0.9940, 1.0031] (20/32; bar 0.995); the six CAVLC streams 0.9795, the 26 CABAC streams 0.9981. Eight threads, ten rounds, discard overall 0.9880 [0.9709, 0.9920] (25/32),
per class sd 0.9825, 720p 1.0147 (bar at most 1.010), 1080p 0.9949, 4k 0.9685; write overall 0.9885 [0.9756, 1.0014].
Against FFmpeg 9.0.2 (median per-stream ratio, share where ours is faster), one thread discard: before 1.123 (7/32), after 1.130 (8/32); write 1.080 (10/32), 1.094 (11/32); eight threads discard 1.017 (14/32), 1.012 (16/32);
write 1.005 (15/32), 1.003 (15/32).
