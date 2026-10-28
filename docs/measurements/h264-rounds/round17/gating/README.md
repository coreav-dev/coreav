# Round 17 gating run, cloud container

before main `06141b3` (main moved to `814eb2a` during the request through documentation commits only: `git diff --stat
06141b3 814eb2a -- crates` is empty, so the decoder is the same and no rebase was done), after `perf/h264-banded-mc` `a9bfa7b`.
Built on the A/B helper's VM (`dist`, `--locked`, rustc 1.99.1), copied unchanged into one container (`binaries.sha256`: before
`f05e63e5...`, after `d330c859...`). `analyze.py` of `a9bfa7b` (`python3 -I analyze.py DIR t8 g`); runner script not published (resumable, preemptible).

**Host class: cloud container, AMD EPYC family 25 model 17 (Zen 4), 32 cores visible, 16 CPUs requested (`nproc` 16),
AVX-512 present (the decoder's AVX-512 tier is active on both arms; the round does not depend on it).** Reference for this
host: three samples five minutes apart (`log.txt`: max deviation 0.6% and 0.5%). Identity 32 of 32 (after at 1 and 8 threads
against before at 1, `identity.txt`). Eight calls between gate checks (`t8`: four unpinned `--threads 8 --rounds 5` calls, FFmpeg
also at eight threads, the primary; `g`: four pinned `--pin 2 --rounds 5` calls): 16 passes, no failed check, no void, no
resume, **REBASE count 0** (`events.json`). Elapsed 3,022 s. Earlier launches of this run were lost to a VM suspend (the client
process died with it; no measurement had started) and to three stability failures on other hosts (`earlier-attempts.txt`), none of
which timed anything.

Result (`analysis.txt`): `bars: B-1 True, B-2 True, B-3 True`. After over before: eight threads discard overall 0.9526 [0.9441,
0.9615] (30/32 faster), write 0.9774 [0.9651, 0.9867]; one thread discard 0.9974 [0.9935, 1.0004], write 0.9958 [0.9914, 0.9991]. The
ratios to FFmpeg 9.0.2 are this host's (base `06141b3`): eight threads discard 1.166 before, 1.133 after; one thread 1.219 and 1.208.
