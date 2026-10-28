# Deblocking strengths round: gating run on a cloud container

before main `051e1692` (main did not move), after `perf/h264-deblock-strengths` `fe2e7c6f`. Built on the A/B helper's VM (`dist`, `--locked`, rustc 1.99.1), copied unchanged into
one container (`binaries.sha256`: before `5c14b670...`, after `cbbb3f24...`). `analyze.py` of `fe2e7c6f` (`python3 -I analyze.py DIR g t8`); runner script not published.

**Host class: cloud container, AMD EPYC family 25 model 17 (Zen 4), 32 cores visible, 16 CPUs requested, AVX-512 present (active on both arms).** Reference for this host: three
samples five minutes apart (`log.txt`: max deviation 4.3% and 4.7%, inside +-5%); a first attempt failed the stability check on another host and timed nothing. Identity 32 of 32 (after at 1
and 8 threads against before at 1). Eight calls between gate checks (`g`: four pinned `--pin 2 --rounds 5`; `t8`: four unpinned `--threads 8 --rounds 10`): the `g.720p` call was **voided once**
(an after-check failed) and run again after the host settled (`events.json`: one VOID; the voided files are not kept in the volume copy but are in `log.txt`), 17 passes and 8 failed
checks in all (the failed ones waits and the void), no resume, **REBASE count 0**. Elapsed 5,326 s.

Result (`analysis.txt`): `bars: Z-1 True, Z-2 True, Z-3 True, Z-4 True`. After over before, one thread pinned: discard overall 0.9880 [0.9808, 0.9969] (24/32 faster), write overall 0.9896
[0.9806, 0.9959]; discard per class sd 0.9910, 720p 0.9940, 1080p 0.9852, 4k 0.9880 [0.9529, 1.0078]. Eight threads, ten rounds, discard overall 0.9896 [0.9824, 0.9959] (26/32).
