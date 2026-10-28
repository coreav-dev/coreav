# Audit: FFmpeg's `-f rawvideo` without `-fps_mode passthrough`

Evidence for docs/design/h264.md, "FFmpeg write timing without `-fps_mode passthrough`: the audit". Host: the A/B helper's
development VM (Intel Xeon family 6 model 106, 2.90 GHz); FFmpeg 9.0.2 BtbN `n9.0.2-17-g2a571b6068` and 7.0.2 static
(`/opt/coreav-oracle-bin/ffmpeg`).

- `vsync-heldout.txt`: `../snapshot-outliers/vsync.py` on the 64 held-out streams (`../../h264-n2-heldout/list.json`,
  extracted with that directory's `tools/extract.py` for this check only: no timing of the draw, no tuning), `main` at
  `a9017ba`'s `decode` example. Two differ. The training set's is `../snapshot-outliers/vsync.txt`.
- `factors.py`, `factors.json`: FFmpeg alone on the six streams, default against passthrough, five interleaved rounds
  pinned to core 2, between host gates (the first run's closing gate failed once on its 1080p stream and passed
  a minute later; the run was repeated, and `factors.json` is the repeat, whose gates both passed).
- `correct_reports.py`, `corrected-reports.txt`: the committed reports' `:write` ratios of the six streams divided by
  their factor, medians before and after. `heldout-medians.txt`: the held-out draw's `:write` medians before and after,
  overall and per class, for both FFmpeg versions.
