# Motion round: fewer or cheaper requests for the reference's lines (not queued)

`docs/design/h264.md`, "Motion round: fewer or cheaper requests for the reference's lines". The variants are on
`rejected/h264-request-variants` at `ec285f51`, based on `main` at `3d60f0bf`, each selected at build time by an
environment variable (`RQ_LEAN`, `RQ_SPREAD`, `RQ_AFTER`, `RQ_FAR`, `RQ_DIST`, `RQ_T1`, `RQ_C8`); with none set the
build runs `main`'s logic (screen 1's control ran 831 more instructions than `main` on 30 pictures of
`1d4b1775d78a`; at the branch's head `request` reaches it through one more call). `RQ_LEAN` changes the walk the destination's write hints share, and adds a row bound to the
position check. Every file here comes from a development VM of the 2.90 GHz class (Intel Xeon @ 2.90 GHz,
family 6, model 106); none is a bar run.

- `profile-4k.txt`: where `Planes::request`'s time and instructions go on `main` (perf, one 4k stream; callgrind
  instruction and call counts on a 720p and a 4k stream).
- `screen1.jsonl` to `screen3.jsonl`: `luma8x8/ab.py` (interleaved CPU time, one thread pinned to CPU 2,
  `--discard`, the class frame counts, five rounds) with `ES` set to the 24 training streams of the 720p, 1080p and
  4k classes (the sd class issues no request); each screen's control, `base`, is a build of the same branch with no
  variable set.
- `screen1.txt` to `screen3.txt`: `python3 -I an2.py FILE base ARM...`, the median per-stream ratio over `base` with
  its 95% bootstrap interval, overall and per class, and how many streams are faster. `oldbase` in screen 2 is
  screen 1's control, built from the branch before the switches of screens 2 and 3 were added: the same logic,
  so its ratio to `base` is the local noise between two builds.
