# Chroma macroblock kernel round: gating run on the A/B helper's Intel VM

before main `051e1692` (main did not move), after `perf/h264-chroma-mb` `1547f768`. Built on this VM (`dist`, `--locked`, rustc 1.99.1;
`binaries.sha256`: before `5c14b670...`, after `52bfb136...`). `analyze.py` of `1547f768` (`python3 -I analyze.py DIR g t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads (`log.txt`); every call ran with `--require-cpu "Intel(R)
Xeon(R) Processor @ 2.90GHz"`.** The runner (script not published): a fresh reference
from three samples five minutes apart (max deviation 1.6% and 0.5%), identity 32 of 32 (after at 1 and 8 threads against before at 1), then eight calls
between gate checks (`g`: four pinned `--pin 2 --rounds 5`; `t8`: four unpinned `--threads 8 --rounds 10`): 16 passes, no failed check, no void,
no resume, **REBASE count 0** (`events.json` is empty). Elapsed 3,687 s. Spend: none (no cloud host use).

Result (`analysis.txt`): `bars: CM-1 True, CM-2 True, CM-3 True, CM-4 True`. After over before, one thread pinned: discard overall 0.9925
[0.9875, 0.9947] (31/32 faster), write overall 0.9918 [0.9864, 0.9950] (28/32); per class discard sd 0.9870, 720p 0.9909, 1080p 0.9969, 4k 0.9934.
Eight threads discard overall 0.9942 [0.9863, 1.0045] (20/32).
