# Deblocking setup round: gating run on the A/B helper's VM

Arms: `base` = main `70a4cec`, `k` = `e03cf436` (`perf/h264-deblock-setup`). Built on the helper's VM (`dist`, `--locked`, rustc 1.99.1), `binaries.sha256` (base `fde0d5d0...`, k `37eac56b...`). `run-ab-helper.sh` and `analyze.py` of the
branch (`2113d122`); FFmpeg 9.0.2 `0d709a0b...`. Bars DS-1 to DS-5 were committed before the run; the harness options are the method's (`--shuffle-seed 20261026 --null-arm base --gate-stream 41e324ebb708.0.sd.h264 --gate-every 4`;
eight-thread calls with `--min-seconds 3`).

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity 32 of 32 (k at 1 and 8 threads against base at 1).

What was run, in order (nothing is dropped):

1. A first run of an earlier pair (`b366429` against `a65f2dc`, the request was replaced): stopped during its first call; its partial files are in `voided-old-pair/` and were not analysed.
2. The `70a4cec`/`e03cf436` run with `run-ab-helper.sh`: the four one-thread calls (`g.*`, pinned `--pin 2 --rounds 20`, harness of `70a4cec`) completed and are the ones in this directory. Its first eight-thread call (`t8.sd`, harness of an earlier change)
   read the in-call gate's cold first sample (0.147 s) as the baseline and every later sample as -21% to -26%, so every stream was timed again and ended unresolved: a harness defect, no bar result; stopped after about an hour, its files are in `t8-aborted-1/`
   and were not looked at for ratios. Fixed in an earlier change (the baseline is taken after a discarded warm sample).
3. The eight-thread calls again with the harness of main `acf3ee2f` (`run-t8.sh`): its first call (`t8-aborted-2/`) showed the gate stream of 300 sd frames (0.12 s, one thread unpinned) reading +10.9% and -6.2% in one call (the noise of a short run), so the
   gate stayed the same stream at 1800 frames, one thread on core 2, repeated to 3 s (`--gate-frames 1800 --gate-threads 1 --gate-pin 2`), and the eight-thread calls were run again (`t8.*`, the ones in this directory). This was a harness change, not a
   change after seeing any ratio.

Host gate (`hostgate.log`): 18 passes and 3 failed checks (each waited and passed), no void call, **REBASE count 0**. Method lines (`analysis.txt`): one thread, `g.sd` 1 unresolved, `g.1080p` 1 unresolved (2 of 32 in all; the harness's rule for a void
one-thread call is more than 2), 4+4+1+1 streams timed again; eight threads: none unresolved, 2 timed again, largest null deviation 6.4% (bar 8% at eight threads), no gate drift. Spend: none (local VM).

Result (`analysis.txt`): `DS-1 pass, DS-2 pass, DS-3 pass, DS-4 pass` (DS-5 is read from `identity.txt` and the conformance report). `k` over `base`, one thread pinned, discard: overall 0.9848 [0.9803, 0.9935] (27/32 faster), write 0.9857 [0.9810, 0.9925]
(25/32); per class discard sd 0.9805, 720p 0.9782, 1080p 0.9943, 4k 0.9945. Eight threads, ten rounds, discard overall 0.9836 [0.9724, 0.9951] (23/32), write 0.9900 [0.9851, 0.9954]; per class discard sd 0.9726, 720p 0.9659, 1080p 1.0072, 4k 0.9907.
Against FFmpeg 9.0.2 (median per-stream ratio, base then k; share where ours is faster): one thread discard 1.097 (9/32) then 1.085 (10/32), write 1.055 (10/32), 1.037 (14/32); eight threads discard 1.009 (15/32), 0.969 (20/32), write 0.989 (19/32), 0.975 (19/32).
The eight-thread numbers are of this host (8 vCPUs, SMT on), whose same-binary median noise is about 1.5% either side.
