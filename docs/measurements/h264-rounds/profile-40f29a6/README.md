# Profile of `main` at `40f29a6` against FFmpeg 9.0.2, one thread, discard

The method of `../profile/README.md`, again (`docs/design/h264.md`, "Profile: where the time goes against FFmpeg", "Again at
`40f29a6`"). Host: development VM, Intel Xeon @ 2.90 GHz, family 6, model 106; software `cpu-clock` at 4 kHz, pinned to one CPU.

- CoreAV `main` `40f29a6` built `dist` with line tables only, the binary named `prof-40f29a6` (the classifier finds its
  samples by that name); FFmpeg's timing build is BtbN `n9.0.2-17-g2a571b6068`, recorded in the same run with this
  directory's `rec.py` (`82c7f7b`'s, `ffmpeg -threads 1 -f null -`; the write-timing audit found `-f null` unaffected by
  `-fps_mode`). Raw samples: `samples.tar.gz` on branch `measured/h264-profile-40f29a6`.
- FFmpeg's stages: the attribution samples of `../profile/` (`samples.tar.gz` on branch `measured/h264-profile`: the
  `*.ff.*.json` of the unstripped 9.0.2 release build `ffmpeg_g` and its `addr.ffmpeg_g.json`; `ffmpeg_g` built as
  `../profile/README.md` says), by its symbol table and debug information alone; no source read.
- `classify.py` is `../profile/classify.py` with two of the decoder's functions added since `82c7f7b` (`predict_fast`,
  `reference_band`) under inter prediction, and the binary's name; `stages-old-classifier.txt` is the same run through
  `82c7f7b`'s list. `items.py` normalises each stream's items to the attribution build's per-stream total of `82c7f7b`'s
  run (not to this run's BtbN times, as the stage table does), so items read about 5% below the stages.

```
python3 -I rec.py ES_DIR run40 prof-40f29a6 BTBN_FFMPEG 2          # run40/<stream>.<class>.{core,ff}.<round>.json
python3 -I resolve2.py run40 prof-40f29a6 prof-40f29a6              # run40/addr.prof-40f29a6.json
python3 -I classify.py run40 FF_ATTRIBUTION_DIR ffmpeg_g > stages.txt  # FF_ATTRIBUTION_DIR: ../profile's ff samples
ROUND=0 python3 -I classify.py run40 FF_ATTRIBUTION_DIR ffmpeg_g > stages.r0.txt   # likewise ROUND=1
python3 -I items.py run40 FF_ATTRIBUTION_DIR ffmpeg_g mb > items-mb.txt   # entropy inter deblock recon other: items-rest.txt
```

The note's alternative ratios under `084ee40`'s FFmpeg attribution recording are this run's CoreAV column divided by
that profile's FFmpeg column (`docs/design/h264.md`, "Kernel round: a reference's interior, found once").
