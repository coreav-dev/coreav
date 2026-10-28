# Round 25: five small changes together

`docs/design/h264.md`, "Round 25: five small changes together": the composition and the bars BN-1 to BN-5, fixed
before any timing.

- `gating/`: the gating run on the bar-class host (`before` = `main` `3d60f0bf`, `after` = `ddb4aa24`), its
  `README.md`, `analysis.txt`, reports, gate log and identity check. Rejected by BN-1, BN-2 and BN-4; the code is on
  `rejected/h264-small-bundle`.
- `analyze.py`: the bars BN-1 to BN-4 from the A/B run's reports (`python3 -I analyze.py DIR g t8`).
- `local/local-ab.txt` (`local-ab.jsonl`): the local timing A/B that decided whether to request the gating run
  (`../luma8x8/ab.py`, one thread, five rounds, the 32 streams), information, not a bar run.
- `local/identity.txt`: the bundle's output against `main`'s, 32 training streams at 1 and 8 threads, with and without
  `fast`.
- `conformance.txt`: the conformance suite on the bundle, 1, 2 and 8 threads, with and without `fast`, detected and
  scalar kernels.
