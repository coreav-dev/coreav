# Deblocking driver: the per-macroblock setup of the kernels' calls (probe)

`docs/design/h264.md`, "Deblocking round: the kernels' setup in the form the tables give", "What the measurement says".
development VM of the 2.90 GHz class (Intel Xeon @ 2.90 GHz, family 6, model 106). `plan.txt` (the removed set, the
bound and the threshold) was committed before any measurement. No code was written for the probe.

Build `prof-main`: `main` at `145232d` with `CARGO_PROFILE_DIST_DEBUG=line-tables-only cargo build --locked --profile
dist --example decode -p coreav-h264`. All runs `--threads 1 --discard` at the class frame counts.

- `perf.sh`: perf `cpu-clock` at 10 kHz, five runs a stream with `--kernels avx2` (`pd/`) and five with the detected
  kernels (`pdd/`, totals only), pinned to CPU 2.
- `cg.sh`: callgrind `--dump-instr=yes --cache-sim=yes --D1=49152,12,64`, `--kernels avx2`, one run a stream.
- `nmb.sh`: macroblocks a run decodes (pictures up to the class count times the coded size), from ffprobe; `nmb.txt`.
- `bound.py STREAM NMB`: the bound of `plan.txt` for one stream, one JSON line (`bound.jsonl`). Adapted from the MB
  layout probe's `mbs.py` (its readers of perf, callgrind and addr2line output). An instruction belongs to a part by
  the line of the `deblock_row` frame it is inlined into; the kernels' reading of the parameters by function or line.
- `table.py`: `bound.txt`, per stream and per class, with the parts and their instructions a macroblock.

```
D=/tmp/dsp ES=TRAINING ./perf.sh; ls TRAINING/*.h264 | D=/tmp/dsp xargs -P 7 -n 1 ./cg.sh
ES=TRAINING ./nmb.sh > nmb.txt
while read s n; do D=/tmp/dsp python3 -I bound.py $s $n; done < nmb.txt > bound.jsonl
python3 -I table.py bound.jsonl
```

Result: the median bound is 4.23% (sd), 5.33% (720p), 2.04% (1080p) and 0.55% (4k) of the detected-kernel time,
above the 1.5% threshold in three classes, so the setup round was built. Two streams have the filter off and bound
0.00%.
