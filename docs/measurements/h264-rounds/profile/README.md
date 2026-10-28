# Profile of `main` against FFmpeg 9.0.2, one thread, discard

Where CoreAV spends time that FFmpeg does not, by decoding stage (`docs/design/h264.md`, "Profile: where the time goes
against FFmpeg"). Host: development VM, Intel Xeon 2.90 GHz class (family 6 model 106), no hardware counters, so the samples
are software `cpu-clock` at 4 kHz (`perf record -e cpu-clock`), pinned to one CPU.

- `rec.py ES_DIR OUTDIR CORE_BIN FFMPEG_BIN ROUNDS`: records CoreAV `main` `82c7f7b` (`dist`, line tables only) `--discard` and `ffmpeg -threads 1 -f null -` on the
  32 training streams with the class frame counts (sd 1800, 720p 900, 1080p 300, 4k 150), two rounds; `rec2.py` the same
  for FFmpeg alone. Raw samples: `samples.tar.gz` on branch `measured/h264-profile`.
- FFmpeg binaries: the BtbN build `n9.0.2-17-g2a571b6068` (the timing reference of every round) and, for attribution
  only, the 9.0.2 release configured with `--disable-everything --enable-decoder=h264 --enable-parser=h264
  --enable-demuxer=h264 --enable-muxer=null,rawvideo --enable-encoder=rawvideo,wrapped_avframe --enable-protocol=file,pipe
  --enable-filter=null,format,scale --disable-stripping` (asm on, nasm 2.15, gcc 11.5.0; `ffmpeg-9.0.2.tar.xz` sha256
  `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`), built with `make` with its output sent to a log
  that was not read, and the source never opened: stages come from its symbol table and debug information. Its output
  is the BtbN build's (raw frames of `9db41aa16b5a`, 300 pictures: md5 `e326636fe331266aaebce7b6319e9963` from both);
  its time is 0.950 of BtbN's, median per stream (`ffmpeg-builds.txt`), so its stages are used as fractions and every
  time is scaled to BtbN's, which spreads BtbN's extra time (startup weighs on the short streams) over every stage.
- Run-to-run noise: a stream's CPU time differs by up to a factor of 1.6 between the two rounds on this VM (steal time
  is likely), for both decoders; the stage shares hold across the rounds (`table.r0.txt`, `table.r1.txt`). These are
  sampled CPU-time ratios, not the rounds' wall-clock A/B metric.
- `resolve2.py`: each sampled address to its inline chain (`llvm-addr2line --inlines`). `classify.py`: stage of a sample
  and the stage table (`table.txt`; `table.r0.txt` and `table.r1.txt` each round alone). `items.py`: the items within a
  stage (`items.txt`, normalised to the attribution build's total). `inter_split.py`: inter prediction's parts
  (`inter_split.txt`).
- `edge-counts.txt`: how many luma predictions from a whole reference take the edge path, from a counting build of `main`
  (not for merge), and how many a replicated border of 16, 32 or 64 samples would keep inside.
- Whether a cloud container exposes `perf_event_open` (hardware cycles, cache misses) was probed once: it does not (gVisor, `ENODEV` for every event). The probe script is not published.
