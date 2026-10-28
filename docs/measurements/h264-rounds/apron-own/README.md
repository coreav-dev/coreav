# The aprons' rows owned ahead (`prefetchw`)

The round of `docs/design/h264.md`, "Threading round: the aprons' rows owned ahead" (bars AW-1 to AW-5, committed
before any timing).

- `identity.txt`: `k` byte-identical to `base` at 1 and 8 threads on the 32 training streams (AW-5; conformance as
  `../apron-cut/conformance/`, the copies being the same, was not re-run: the result below ended the round first).
- `run-gcp.sh`, `own1/` (`ac-summary.txt` by `../apron-cut/ac.py`): eight threads on Compute Engine.
- `run-1t.sh`, `t1/`: one thread pinned, six rounds (AW-4).
- `instructions.txt`: callgrind counts at one thread for `base`, this `k` and round 3's form.
- `t1-info/`: an information run after the result, three builds (`base`, this `k`, the field-free form of the next
  round), not a bar's run.

Result: eight threads 0.9941 [0.9912, 0.9970] (sd 0.9861), AW-1 to AW-3 met; one thread write 1.0031, AW-4 missed
(the decoder's field layout); rejected. Spend 1.61 USD (bound).
