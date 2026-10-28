# Round 13 re-measure, on a cloud host, under the host-health gate

Builds: base main `9cc143d`, g `perf/h264-mc-glue-2` `14435e9`, r `perf/h264-mc-ref-13b` `a445499`, gr `perf/h264-mc-glue-r`
`22c6fbd` (built on the A/B helper's VM, `dist`, `--locked`, rustc 1.99.1; binary SHA-256 in `binaries.sha256` and
`provenance-builds.txt`; `main-at-start.txt` is the main of the local attempt). The decoders were copied unchanged into one
cloud container.

**Host class: cloud container, AMD EPYC family 25 model 1 (Zen 3), 24 cores visible, one thread per core (`cpu.txt`), 8 CPUs
requested (the visible-core count is 24; `nproc` 8; no cgroup `cpu.max`), no AVX-512 (`avx2` only): the AVX-512 tier is
inactive on every arm.** The class qualifies for rounds that do not depend on an ISA tier. Both arms and FFmpeg 9.0.2 are
in the same container, in the same interleaved rounds. Absolute ratios to FFmpeg are this class's (base 1.347 discard); the
bars are within-run ratios.

Method, as described in `host-gate/HOST-GATE.md`: a fresh reference for this host from three gate
measurements five minutes apart (`reference.json`: max deviation 1.4% and 0.5%, stable within +-5%), identity (32 of 32,
g, r and gr at 1 and 8 threads against base at 1, `identity.txt`), then each timing call between two gate checks, run 1
pinned (`--pin 2 --rounds 5`, four builds) and run 2 at eight threads (`--threads 8 --rounds 3`, base and g), `analyze.py` of
`22c6fbd`. `log.txt` is the container's log: 16 gate passes, 4 failed before-checks on `t8.sd` (the host was 6 to 17% slow
for about five minutes, the call waited), no call voided. Elapsed 6,050 s.

Result (`analysis.txt`): G earns its place, R does not (gr over g 1.0067 on discard), so the bars are read for g:
R13-1, R13-2, R13-3 and R13-4 all hold. g over base: discard 0.9878 [0.9822, 0.9928] (25/32 faster), write 0.9881
[0.9844, 0.9928]; large classes discard 0.9906 [0.9693, 1.0017]; eight threads g over base discard 0.9830 [0.9691, 0.9988].
R alone is a loss here (r over base discard 1.0237 [1.0081, 1.0314]).
