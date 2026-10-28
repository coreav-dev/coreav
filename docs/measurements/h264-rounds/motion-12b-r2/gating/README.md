# Motion round: a block's motion in 12 bytes, on the current decoder: gating run on the Intel 2.90 GHz VM

before main `72b173b5` (main did not move), after `perf/h264-motion-12b-r2` `c8339d9c`. Built on this VM (`dist`, `--locked`, rustc 1.99.1; `binaries.sha256`: before `5997ede9...`, after `a6252210...`).
`analyze.py` of `c8339d9c` (`python3 -I analyze.py DIR g gm t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads; every call ran with `--require-cpu "Intel(R) Xeon(R) Processor @ 2.90GHz"`.** Runner (script not published): fresh reference from three samples five minutes apart (max deviation 0.6% and 0.04%), identity 32 of 32 (after at 1 and 8 threads against before at 1), twelve calls between
gate checks (`g`: four pinned `--pin 2 --rounds 5`; `gm`: the same with `MALLOC_MMAP_THRESHOLD_=131072` in the environment of the whole harness call, so of both decoders **and FFmpeg**; `t8`: four unpinned `--threads 8 --rounds 10`):
24 passes, one failed check that only caused a wait, no void, no resume, **REBASE count 0**. Elapsed 5,598 s. Spend: none.

Result (`analysis.txt`): `bars: MR-1 True, MR-2 True, MR-3 True, MR-4 True, MR-5 True` (MR-6, output, is the identity and the conformance report). After over before: one thread pinned discard overall 0.9907 [0.9866, 0.9965]
(29/32 faster), write 0.9907 [0.9857, 0.9967]; per class discard sd 0.9953, 720p 0.9917, 1080p 0.9897, 4k 0.9633 [0.9503, 0.9960]. Mapped discard 0.9912 [0.9844, 0.9970], write 0.9946. Eight threads discard 0.9893 [0.9799, 0.9942]
(26/32), 4k 0.9520.
