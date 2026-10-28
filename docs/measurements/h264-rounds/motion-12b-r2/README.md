# Motion round: a block's motion in 12 bytes, on the current decoder

`docs/design/h264.md`, "Motion round: a block's motion in 12 bytes, on the current decoder" (a replication of
`../motion-12b/` on `main` at `72b173b5`).

- `local/profile-stages.txt`: stage table of `main` at `d7bb7aeb` against FFmpeg 9.0.2, one thread, discard
  (`../profile/rec.py`, `classify.py`; the development VM of the 2.90 GHz class, two rounds).
- `local/profile-mb-items.txt`: the macroblock layer's items in that profile (`../profile/items.py ... mb`).
- `local/instructions.txt`: callgrind with its cache model, `main` at `d7bb7aeb` against the branch on it.
- `local/local-timing.txt`: local wall times, information only.
- `analyze.py`: the bars MR-1 to MR-5 from the A/B run's reports (`python3 -I analyze.py DIR g gm t8`).
- `identity.txt`, `conformance.txt`: the branch's output against `main`'s, and the JVT suite, on the rebased commit.
- `identity-rebased.txt`: the output after the rebase onto `93f7d870`.
- `gating/`: the A/B run of the bars MR-1 to MR-5 (its README, `analysis.txt`, `binaries.sha256`, identity, the JSON and
  logs of each call; from branch `measured/h264-motion-12b-r2` `e611de4e`). Its `analyze.py` was this directory's at
  `c8339d9c`, the same file, and is not repeated there.
