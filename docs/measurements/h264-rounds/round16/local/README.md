Local A/B runs on the development VM (2.90 GHz class, noisy: information only, not the gating run). CPU time (user and
system) of `--discard` at one thread, pinned, the class frame counts, 7 rounds, builds alternated or rotated
(`ab.py`, `ab3.py`; `ES` and `BINS` name the stream and binary directories). Ratios are per stream, median over rounds.

- `ab-upper-bound.txt`: `main` `82c7f7b` against a build whose edge windows are clamped into the picture so that every
  block takes the fast path (not bit-exact: an upper bound on what removing the edge path can give).
- `ab-border.txt`: the replicated border prototype (`exp/h264-ref-border` `61cf164`) against `82c7f7b`.
- `ab-border-widths.txt`: borders of 32 (with the fill skipped for non-reference pictures, `e9bfa7c`), 64 and 128 luma
  samples against `82c7f7b`.
- `ab-tight.txt`: this round (`6c519fc`) against `main` `3ebeda9`.
