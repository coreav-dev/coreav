# Threading round: a finished worker takes the next picture

The evidence of the section of that name in `docs/design/h264.md`. Rejected by R1 to R3; the code is on
`rejected/h264-retire-any` at `7f947e39`.

- `analyze.py`: the summary of the gating runs and bars R1 to R4. Its B group is the 17 training streams with
  B pictures.
- `local/r5.txt`: bar R5's identity, conformance and memory test on the measured commit.
- `gating/`: the gating run on a cloud container (branch `measured/retire-any` `ab6a80e3`): host, gate, reports,
  `identity.txt`, `analysis.txt` and the CPU time at eight threads.
