# Round 16 gating run, cloud container

before main `3ebeda9`, after `perf/h264-tight-windows` `d8bf74e`. Built on the A/B helper's VM (`dist`, `--locked`, rustc 1.99.1), copied
unchanged into one container (`binaries.sha256`: before `e0141772...`, after `a749b076...`). `analyze.py` of `d8bf74e`
(`python3 -I analyze.py DIR g t8`); runner script not published.

**Host class: cloud container, AMD EPYC family 26 model 2 (Zen 5), 32 cores visible, 16 CPUs requested (`nproc` 16), AVX-512
F/BW/CD/DQ/VL/VBMI present (the decoder's AVX-512 tier is active on both arms; the round does not depend on it), `nonpreemptible`
function.** The run's reference is fast (0.961 s and 3.312 s for the two gate streams), so absolute ratios to FFmpeg are this
host's (base 1.166 discard). Reference from three samples five minutes apart (`reference.json`, max deviation 0.6% and 0.4%);
earlier attempts (`earlier-attempts.txt`) failed the stability check on other hosts and timed nothing. Identity 32 of 32 (after at 1
and 8 threads against before at 1). Eight calls between gate checks (`g`: four pinned calls, `--rounds 5`; `t8`: four unpinned
`--threads 8 --rounds 3` calls): 16 passes, no failed check, no void, **REBASE count 0**. Elapsed 2,448 s.

Result (`analysis.txt`): `bars: T-1 True, T-2 True, T-3 True, T-4 True`. After over before, one thread pinned: discard overall 0.9882
[0.9830, 0.9972] (22/32 faster), write overall 0.9925 [0.9858, 0.9974] (23/32); per class discard sd 0.9675, 720p 0.9993, 1080p
1.0013, 4k 0.9885. Eight threads, discard, all 32 streams: 0.9899 [0.9828, 0.9948] (27/32 faster).
