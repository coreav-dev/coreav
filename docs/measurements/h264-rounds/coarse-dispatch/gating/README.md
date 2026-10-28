# Kernel round: the hot paths compiled twice: gating run on the Intel 2.90 GHz VM

Four arms built on this VM (`dist`, `--locked`, rustc 1.99.1, separate `CARGO_TARGET_DIR` each; `binaries.sha256`): `main` = `40f29a6d`, `RUSTFLAGS` unset (`16bc3dcb...`); `v3` = `40f29a6d`,
`RUSTFLAGS='-C target-cpu=x86-64-v3'` (`90ca6f39...`); `after` = `def6f84a`, default features (`b2877469...`); `safe` = `def6f84a` with `--no-default-features` (`5e99a742...`). `analyze.py` of `def6f84a`
(`python3 -I analyze.py DIR g gm t8`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106, 4 cores x 2 threads; every call ran with `--require-cpu "Intel(R) Xeon(R) Processor @ 2.90GHz"`** (the gate passed before the run).
Runner (script not published): fresh reference from three samples five minutes apart (max deviation 0.7% and 0.5%), identity 32 of 32 (after and safe at 1 and 8 threads against main at 1), twelve calls between
gate checks (`g`: four pinned `--pin 2 --rounds 5`, decoders main, v3, after, safe; `gm`: the same flags with `MALLOC_MMAP_THRESHOLD_=131072` in the environment of the whole call, decoders main and after, so FFmpeg
too; `t8`: four unpinned `--threads 8 --rounds 10`, decoders main, v3, after): 24 passes, one failed check that only caused a wait, no void, no resume, **REBASE count 0**. Elapsed 7,182 s. Spend: none.

Result (`analysis.txt`): `bars: CD-1 True, CD-2 True, CD-3 True, CD-4 True, CD-5 False, CD-6 True, CD-7 True, CD-8 True`. One thread pinned, discard overall: after/main 0.9730 [0.9634, 0.9774] (29/32 faster), v3/main 0.9617
[0.9540, 0.9672], after/v3 1.0138 [1.0084, 1.0201], safe/main 0.9995 [0.9950, 1.0037], after/safe 0.9726 [0.9648, 0.9817]; write after/main 0.9765. Mapped after/main discard 0.9747. Eight threads after/main discard 0.9792 [0.9575, 0.9927], after/v3 1.0011.
