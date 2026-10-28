# Gating run: the chroma kernel round

`run-ab-helper.sh 3ebeda9 04f26d6` (the script beside `analyze.py` one directory up), on this
development VM: Intel Xeon @ 2.90 GHz, family 6 model 106, AVX-512 (`provenance.txt`). `base` = `main`
at `3ebeda9`; `k` = the branch's code commit `04f26d6`. Both built alike (`dist`) from one worktree
each. Output identical on 32 of 32 streams for `k` at 1 and 8 threads against `base` at 1
(`identity.txt`); the host gate passed before and after each of the four calls, none void
(`hostgate.log`); `analysis.txt` is `analyze.py`'s output, which reproduces from the `g.*.json`
reports. `conformance.txt`: the conformance suite on the branch (code identical to `k`).
