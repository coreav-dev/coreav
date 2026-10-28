# MB layer: a layout that stores only what later stages read (probe)

`docs/design/h264.md`, "MB layer: a layout that stores only what later stages read, a probe". Development VM of the 2.90
GHz class (Intel Xeon @ 2.90 GHz, family 6, model 106). `plan.txt` (the bound and the threshold) was committed before
any measurement. No code was written for the probe.

Build `prof-main`: `main` (crates as `41ae60dc`) with `CARGO_PROFILE_DIST_DEBUG=line-tables-only cargo build --locked
--profile dist --example decode -p coreav-h264`. Both runs use `--kernels avx2 --threads 1 --discard` at the class
frame counts.

- `perf.sh`: perf `cpu-clock` at 10 kHz, five runs a stream, pinned to CPU 2 (`pd/STREAM.R.data`).
- `cg.sh`: callgrind `--dump-instr=yes --cache-sim=yes --D1=49152,12,64`, one run a stream (`cg/STREAM.out`).
- `nmb.py`: macroblocks in a callgrind run (calls into the hot-path instance of `start_mb`); `nmb.txt`.
- `mbs.py STREAM NMB`: the bound of `plan.txt` for one stream, one JSON line (`bound.jsonl`). Per source line of our
  code (innermost CoreAV inline frame), sampled time against callgrind's instructions, data writes (times the store's
  width from the disassembly) and simulated D1 write misses. The record initialisation is matched by its lines in
  `mb.rs` at `41ae60dc` (694 to 698); `put_row` counts toward R when its caller is `put_rows` or `store_mb` (the
  picture's motion), not `set_row` (the slice's motion context). Caches (`*.ips`, `*.pkl`, `prof-main.a2l`,
  `prof-main.widths`) are shared, so run it one stream at a time.
- `table.py`: `bound.txt`, per stream and per class; its last lines add the bound without the share of other
  lines' excess, R's write misses and the share of macroblocks stored with one motion (`mbs.py`'s `one_motion`: the
  bytes of `store_mb`'s copy of rows 1 to 3, `inter.rs` 569 at `41ae60dc`, over 144 a call).
- A line counts as storing where callgrind counts a data write on it; Valgrind also counts some register-only
  instructions (`bts`) as writes, which adds lines to the store excess (about 0.1 point of the bound on the stream
  checked).

```
./perf.sh; ls TRAINING/*.h264 | xargs -P 7 -n 1 ./cg.sh   # both name /tmp/mbs as D, perf.sh the training directory
for f in cg/*.out; do echo "$(basename $f .out) $(python3 -I nmb.py $f)"; done > nmb.txt
D=/tmp/mbs; while read s n; do D=$D python3 -I mbs.py $s $n; done < nmb.txt > bound.jsonl
python3 -I table.py bound.jsonl
```
