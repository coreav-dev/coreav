# Retire-any threading round: gating run on a cloud container

base main `5dcfed70`, a `h264-retire-any` `80dfef38`, pre `e9555171` (main before an earlier change, attribution only). Built on the A/B
helper's VM (`dist`, `--locked`, rustc 1.99.1), copied unchanged into one container (`binaries.sha256`: base `b7fbd511...`, a
`ead88714...`, pre `83de8e5e...`). `analyze.py` of `80dfef38`, `cpu8.py` of `405ef0ef`; runner script not published.

**Host class: cloud container, AMD EPYC family 26 model 2 (Zen 5), 32 cores visible, 16 CPUs requested (`nproc` 16, no quota
below the eight threads), AVX-512 F/BW/CD/DQ/VL/VBMI present, `nonpreemptible` function.** All three builds and FFmpeg 9.0.2 in the
same calls. Reference from three samples five minutes apart (`reference.json`: max deviation 0.6% and 0.6%). Identity (a at 1,
2, 3 and 8 threads against base at 1): 32 of 32 (`identity.txt`). `t` run (`--threads 1,8`, unpinned, `--rounds 5`) and `p` run
(`--pin 2`, `--rounds 5`), eight calls between gate checks: 16 passes, no failed check, no void, **REBASE count 0**; then `cpu8`
(information). Elapsed 4,973 s.

**A lost first attempt.** An earlier run of the same script (Zen 3 host, preemptible function) finished all four `t` calls and
the `p` sd, 720p and 1080p calls (it had one voided call and one REBASE) and was then terminated by the cloud host's preemption;
the function restarted from scratch and the work was lost (the runner returned results only at the end). Its
partial numbers were never read and are not used. The re-launch used a non-preemptible function.

Result (`analysis.txt`): `R1 FAILS, R2 FAILS, R3 FAILS, R4 passes`. a over base, eight threads, unpinned: discard overall
0.9931 [0.9806, 1.0049] (20/32 faster), sd 1.0056, 720p 0.9901, 1080p 0.9823, 4k 0.9836 [0.9338, 0.9970]; write overall 1.0034
[0.9935, 1.0093]. One thread pinned: discard overall 0.9995 [0.9977, 1.0035], write overall 1.0012 [0.9978, 1.0023]. An earlier change (base)
over pre, one thread pinned, discard overall 0.9970 [0.9945, 0.9996], write 0.9971 [0.9945, 1.0006]. CPU time at 8 threads (`cpu8.*.txt`).
