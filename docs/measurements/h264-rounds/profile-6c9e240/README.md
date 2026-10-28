# Profile of `main` at `6c9e240` against FFmpeg 9.0.2, one thread, discard

The method of `../profile/README.md`, again, as `../profile-40f29a6/` ran it. Host: development VM, Intel Xeon @ 2.90 GHz,
family 6, model 106, stepping 6, 8 CPUs (`/proc/cpuinfo`); software `cpu-clock` at 4 kHz, pinned to CPU 3, nothing else
running. `40f29a6` was recorded again on this host in the same run (`stages-40f29a6-here.txt`), so the change since
that profile is read between two runs of one host.

- CoreAV `main` `6c9e240` and `40f29a6`, each built `dist` with line tables only (default features, so `fast` on) in its
  own detached worktree and target directory, the binaries named `prof-6c9e240` and `prof-40f29a6` (the classifier finds
  samples by that name). FFmpeg's timing build is BtbN `n9.0.2-17-g2a571b6068`, recorded in the same runs with this
  directory's `rec.py` (`../profile-40f29a6/rec.py`, unchanged: `ffmpeg -threads 1 -f null -`), two rounds, the
  `6c9e240` run first and the `40f29a6` run straight after it. Raw samples: `samples.tar.gz` on branch `measured/h264-profile-6c9e240`
  (`run6c/` and `run40/` inside it).
- FFmpeg's stages: the attribution samples of `../profile/` (`samples.tar.gz` on branch `measured/h264-profile`: the
  `*.ff.*.json` of the unstripped 9.0.2 release build `ffmpeg_g` and its `addr.ffmpeg_g.json`), by symbol table and
  debug information alone; no source read. That build's symbol table, which places assembly samples, is not on this
  host (the `ffmpeg_g` here is another configuration, and its symbols put the attribution samples' assembly in filters
  the attribution build does not contain). The FFmpeg column depends only on those samples and that symbol table, so it
  is taken as recorded rather than recomputed: `FF_STAGES` points `classify.py` at the stage fractions in that archive
  (`run1/stages.json`, `run1/stages.r0.json`, `run1/stages.r1.json`, identical to `../profile-40f29a6/stages.json`'s
  FFmpeg column to the last digit), and `FF_ITEMS` points `items.py` at `../profile-40f29a6/items-*.txt` for FFmpeg's
  items. CoreAV's stages are each stream's samples over that stream's BtbN total from this run, as before.
- `classify.py` is `../profile-40f29a6/classify.py` with these changes (`stages-old-classifier.txt` is this run through
  that file's list, the FFmpeg column taken as above):
    - `nb.rs`, the neighbour cache (round 22), is the MB layer.
    - The instances of the hot paths (`tier.rs`, `fast/mod.rs`): a sample whose innermost CoreAV frame is a
      wrapper's own code goes to the stage of what it runs. `predict_fast` and `predict_kernel` are inter prediction;
      `significance_map` and `levels` are entropy; `enter` by the function whose closure it runs, named in the frame (or
      the symbol for a bare closure frame): `Cabac::mvd`, `Cabac::new_in` and `CavlcSyntax::block` are entropy,
      `Choice::level_scale` is other (as `LevelScale::new` in `params.rs` was), and the `mb.rs` functions (`start_mb`,
      `reset`, `decode_cabac`, `decode_cavlc`, `parse_residual_cavlc`) by the `mb.rs` rule, as when they were inline.
    - The kernel `chroma_residual_mb` (new since `40f29a6`) is reconstruction.
    - A function named `f::<T>` (an instance's own symbol, as `decode_mb::<CabacSyntax<V3>, V3>` is now) is matched by
      `f`, as `f<T>` was; before, the name matched as empty and fell to the MB layer. This moves 1.2% of `6c9e240`'s
      CoreAV samples from the MB layer to entropy (`decode_mb`, `parse_residual`) and 0.8% to inter prediction
      (`inter_pred`, `motion_compensate`), and 0.4% of `40f29a6`'s from the MB layer to entropy (`parse_mvd`,
      `parse_residual`); `stages-40f29a6-here.txt` uses the same classifier as `stages.txt`.
  With these, CoreAV samples in no stage ("other", the binary's) are 2.1% of `6c9e240`'s, against 2.6% of `40f29a6`'s
  on this host; with `40f29a6`'s list they were 4.8%.
- `items.py` is `../profile-40f29a6/items.py` with the binary's name and `FF_ITEMS`; items are normalised to the
  attribution build's per-stream total, so they read below the stages, as before.
- Rounds: the CoreAV over BtbN mean is 1.093 in round 0 and 1.118 in round 1 (median 1.139 and 1.136); every stage's
  CoreAV share is within 0.008 of FFmpeg's time between the rounds (`stages.r0.txt`, `stages.r1.txt`). These are
  sampled CPU-time ratios, not the rounds' wall-clock A/B metric.

```
# binaries: in a worktree at each commit
CARGO_TARGET_DIR=/tmp/prof/target-<c> CARGO_PROFILE_DIST_DEBUG=line-tables-only \
  cargo build --locked --profile dist --example decode -p coreav-h264      # copied to prof-<c>
python3 -I rec.py ES_DIR run6c prof-6c9e240 BTBN_FFMPEG 2                  # then: run40 prof-40f29a6
python3 -I resolve2.py run6c prof-6c9e240 prof-6c9e240                      # and run40 prof-40f29a6
A=ATTRIBUTION_ARCHIVE/run1; FD=ATTRIBUTION_ARCHIVE/run1g                    # ../profile's samples.tar.gz, extracted
FF_STAGES=$A/stages.json python3 -I classify.py run6c $FD ffmpeg_g > stages.txt
ROUND=0 FF_STAGES=$A/stages.r0.json python3 -I classify.py run6c $FD ffmpeg_g > stages.r0.txt   # likewise ROUND=1
FF_STAGES=$A/stages.json FF_ITEMS=../profile-40f29a6/items-mb.txt python3 -I items.py run6c $FD ffmpeg_g mb > items-mb.txt
FF_STAGES=$A/stages.json FF_ITEMS=../profile-40f29a6/items-rest.txt \
  python3 -I items.py run6c $FD ffmpeg_g entropy inter deblock recon other > items-rest.txt
CORE_DSO=prof-40f29a6 FF_STAGES=$A/stages.json python3 -I classify.py run40 $FD ffmpeg_g > stages-40f29a6-here.txt
```

`ffmpeg_g` is only read for its program headers when `FF_STAGES` is set. `stages.json` is the stage table of
`stages.txt` as data.
