# The two snapshot outliers: `6f7799768413` and `b41f70a7fa60`

Evidence for docs/design/h264.md, "Snapshot outliers". Snapshot: `main` at `06141b3`, Intel 2.90 GHz class
(`measured/h264-snapshot-06141b3`). This run: `main` at `a9017ba` built as `dist`, the same VM class
(Intel Xeon family 6 model 106), FFmpeg 9.0.2 BtbN `n9.0.2-17-g2a571b6068` as the timing reference, and for attribution
the 9.0.2 release built as in `../profile/README.md` (`--disable-everything` with the h264 decoder, `--disable-stripping`;
its build output was written to a log that was not read and its source was not opened).

- `features.py`, `table.py`, `features.tsv`: the parameter-set and slice-header fields of the 32 training streams,
  read from FFmpeg's `trace_headers` bitstream filter (a black box) over each stream's first 300 pictures.
- `vsync.py`, `vsync.txt`: for each stream, the bytes ours writes and FFmpeg's `-f rawvideo` writes at its default
  frame-rate conversion and with `-fps_mode passthrough`, at the class frame counts, and FFmpeg's dup and drop counts.
  Four streams differ: FFmpeg's default writes 6, 3 and 3 pictures (`3b1b441e23ae`, `6da34958296c`,
  `8ae90cadb5bc`: 32,400,000, 9,331,200 and 1,036,800 bytes of 5.40, 3.11 and 0.35 MB a picture) and, for
  `b41f70a7fa60`, 300 pictures made of 150 decoded ones.
- Bits per picture and per macroblock of the CAVLC streams in the note: from `../../h264-training/es.json` (size and
  frames of each elementary stream) and each stream's macroblock count from its coded size.
- `corrected.sd.json`, `.1080p.json`, `.4k.json` (and `.log`): `scripts/h264_parity.py` with `-fps_mode passthrough`
  (this branch), the four streams of `vsync.txt` that differ (`8ae90cadb5bc` sd; `6da34958296c` and `b41f70a7fa60`
  1080p; `3b1b441e23ae` 4k), the class frame counts, 5 rounds, pinned, `main` at `a9017ba`: write ratios 1.196,
  1.265, 1.296 and 1.082 (the snapshot's 1.311, 1.412, 2.335 and 1.347); `corrected-medians.txt` replaces them in
  the snapshot's per-class write medians. `parity-b41f-passthrough.json`: an earlier 3-round run of the same
  command on `b41f70a7fa60` (write 1.294).
- `stages.txt`, `classify.py`: the stage table of `../profile/classify.py` (patched to skip a class that has no stream
  here) for the two streams, two rounds of `perf record -F 4000 -e cpu-clock`, one thread, pinned, `--discard`
  against `ffmpeg -threads 1 -f null -`; the sample addresses resolved with `../profile/resolve2.py`.
  Absolute times of this sort vary between rounds on this VM by up to a few percent.
- `cavlc-two-level.patch`, `cavlc-two-level-ab.json`: the exploratory prototype of the first fix (coeff_token and
  the other code tables in two levels), not for merge, and its ungated interleaved A/B (`abn.py`, a copy at `../luma-kernel/aligned/abn.py`: 5 rounds, pinned,
  discard, base `main` at `a9017ba`): `6f7799768413` 0.9845; median over the eight streams timed 0.9968
  [0.9932, 1.0011].
- `numbers.txt`: the sample counts, the `perf annotate` figure and the callgrind totals quoted in the note.
