# The aprons' copies at eight threads

An earlier change's cause 2 (`../scaling-8t/`). Probe AP: `plan-probe.md` (plan, committed before the run, and its result),
`run-gcp.sh` and `build.sh` (the run), `identity-probe.txt` (`twice` byte-identical to `base` at 1 and 8 threads;
`none` at 1 thread, and different at 8 as planned), `ap.py` (the summary), `run1/` (the run), `run1.void1/` and
`run1.void2/` (two attempts that created no VM: the project's vCPU quota).
