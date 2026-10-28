# Deblocking: the boundary-strength derivation as a vector kernel (probe)

`docs/design/h264.md`, "Kernel round: boundary strengths in vector registers", "What the measurement says".
development VM of the 2.90 GHz class (Intel Xeon @ 2.90 GHz, family 6, model 106). `plan.txt` (the removed set, the
bound and the threshold) was committed before any measurement. No code was written for the probe.

Build `prof-main`: `main` at `acf3ee2f` with `CARGO_PROFILE_DIST_DEBUG=line-tables-only cargo build --locked --profile
dist --example decode -p coreav-h264`. All runs `--threads 1 --discard` at the class frame counts.

- `perf.sh`: perf `cpu-clock` at 10 kHz, five runs a stream with `--kernels avx2` (`pd/`) and five with the detected
  kernels (`pdd/`, totals only), pinned to CPU 2.
- `cg.sh`: callgrind `--dump-instr=yes --cache-sim=yes --D1=49152,12,64`, `--kernels avx2`, one run a stream, after
  the perf runs.
- `nmb.sh`: macroblocks a run decodes (pictures up to the class count times the coded size), from ffprobe; `nmb.txt`.
- `bound.py STREAM NMB`: the bound of `plan.txt` for one stream, one JSON line (`bound.jsonl`). The setup probe's
  `bound.py` with the removed set of `plan.txt`: an instruction belongs to R by its inline frames in deblock.rs.
- `table.py`: `bound.txt`, per stream and per class, with the parts and their instructions a macroblock.

```
D=/tmp/bsp ES=TRAINING ./perf.sh; ls TRAINING/*.h264 | D=/tmp/bsp xargs -P 7 -n 1 ./cg.sh
ES=TRAINING ./nmb.sh > nmb.txt
while read s n; do D=/tmp/bsp python3 -I bound.py $s $n; done < nmb.txt > bound.jsonl
python3 -I table.py bound.jsonl
```

Result: the median bound is 9.31% (sd), 6.00% (720p), 4.09% (1080p) and 12.22% (4k) of the detected-kernel time,
above the 1.5% threshold in every class, so the kernel round is built. The sampled time share of R (no bound terms)
is 7.21%, 4.73%, 3.19% and 7.25%. One stream (`175433794e02`) has the filter off and bounds 0.00%.

By part, medians of the bound: the path of macroblocks with one motion (`strengths`) 3.22%, 2.89%, 1.56%, 8.31%; the
path where a macroblock or a neighbour carries more than one motion (`split`) 2.88%, 1.38%, 1.31%, 0.26%;
`differing_motion_strength` 1.22%, 0.86%, 0.34%, 0.12%; the test for all zeros 1.08%, 1.01%, 0.46%, 3.14%. That
test is 8 instructions a macroblock and its time is the branch on the result (and the wait for it), which a kernel
keeps; the bound counts it in full, generously.
