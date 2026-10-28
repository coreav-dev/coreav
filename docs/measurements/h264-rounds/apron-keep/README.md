# The aprons' rows owned ahead, the decoder's layout kept

The round of `docs/design/h264.md`, "Threading round: the aprons' rows owned ahead, the decoder's layout kept"
(bars AX-1 to AX-5, committed before any timing).

- `identity.txt`, `conformance/`: AX-5 on `k` (`4f57e2b6`, `main` `c2c72c33` and the change).
- `run-1t.sh`, `t1/`: one thread pinned, six rounds (AX-4).
- `run-gcp.sh`, `keep1/` (`ac-summary.txt` by `../apron-cut/ac.py`): eight threads on Compute Engine (AX-1 to AX-3).

Result: eight threads 0.9983 [0.9939, 0.9996] (sd 0.9871), AX-1 missed; AX-2 to AX-5 met; rejected, kept for a
bundle. Spend 1.72 USD (bound).
