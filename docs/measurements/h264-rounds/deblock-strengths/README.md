# Deblocking round: the strengths of quiet macroblocks

`docs/design/h264.md`, "Deblocking round: the strengths of quiet macroblocks".

- `cache.txt`: first-level cache misses of the deblocking glue at 4k (callgrind's cache model), `main` at `051e1692`.
- `local-prefetch.txt`: local timing of prefetch hints for the records (information, the rejected alternative).
- `instructions.txt`: callgrind instruction counts of the whole decode, `main` against the branch.
- `identity.txt`: the branch's output against `main`'s; `conformance.txt`: the JVT suite on the branch.
- `analyze.py`: the bars Z-1 to Z-4 from the A/B run's JSON files.
- `gating/`: the A/B run of the bars Z-1 to Z-4, from branch `measured/h264-deblock-strengths` `3180d2d4` (its README,
  `analysis.txt`, the JSON and logs of each call, the void 720p call's files, the runner).
