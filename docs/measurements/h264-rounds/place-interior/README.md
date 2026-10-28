# Kernel round: a reference's interior, found once

`docs/design/h264.md`, "Kernel round: a reference's interior, found once". Host: development VM, Intel Xeon 2.90 GHz class
(family 6 model 106).

- `profile/`: the profile of `main` at `084ee40` against FFmpeg 9.0.2, one thread, discard, by the method and scripts of
  `../profile/` (`rec.py`, `rec2.py`, `resolve2.py`, `classify.py`, `items.py`, `inter_split.py`, copied here; the
  recording scripts now pass `-fps_mode passthrough` to FFmpeg, which `-f null` does not use). Outputs: `table.txt` (by
  stage, per class and over all 32), `inter_split.txt`, `items.txt` (MB layer, reconstruction, inter, entropy and
  deblocking items over all 32), `inter_items_by_class.txt`, `recon_split.txt` (intra, transforms, scaling, glue),
  `cabac_split.txt` (CABAC residual against other CABAC by inline chain; FFmpeg's out-of-line decisions are not
  attributable to their callers, so read it with care), `predict_items.txt` and `predict_lines.txt` (inside
  `Kernels::predict`'s x86 form: by innermost function and by line). New scripts: `stageitems.py`, `recon_split.py`,
  `chainsplit.py`, `kitems.py`, `klines.py`. `hostgate.log`: the gate around the recording.
- `instructions.txt`: callgrind totals of `main` and of the round's code commit on one 1080p stream, and FFmpeg's
  motion-compensation functions on the same stream (function totals only).
- `run-ab-helper.sh BASE K` and `analyze.py`: the gating run and its analysis (bars PI-1 to PI-5), committed before it.
- `verification.txt`: the tests of the round's code commit (`place` with the interior against the window test
  alone, the interior's sufficiency, its emptiness for small planes), their mutation check, and a fuzz run of the
  `predict` target.
- `gating/`: the run (`run-ab-helper.sh 084ee40 42a5388`): `analysis.txt` and `summary.json` (bars PI-1 to PI-4),
  `identity.txt`, `conformance.txt` (PI-5), `hostgate.log`, `provenance.txt`, the per-call reports and logs; `REBASE.md`,
  `rebase-checks.log` and `hostgate-rebased.py` (the level shift applied before the restart); `first-start/` (the start that
  stopped, with its three voided reports, kept as recorded and not read).
- `rebased/`: the run after the rebase onto `051e169` (bars RB-1 to RB-4, committed in fa8a715 before it): the same files as `gating/`.
