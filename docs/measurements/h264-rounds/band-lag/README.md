# Threading round: a band published when its rows are filtered

The evidence of the section of that name in `docs/design/h264.md`.

- `profile/`: the cause, measured on a 2.50 GHz host (Intel Xeon, family 6, model 85; 4 cores,
  8 logical CPUs), not the bar class. `diag.patch` is the diagnostic build on `main` at 05daabc4
  (timers around a worker's message waits, its slices, its band waits and the caller's waits; a
  histogram of each band wait by the band's first row against the row being decoded; an `EARLY`
  switch for a what-if that publishes bands before they are final, not used in the note).
  `diag-main.txt` is that build at 8 threads on the eight sd streams, median of three runs;
  `diag-branch.txt` the same timers on the branch with bands of one row, before pictures that are
  not references became one band.
- `local/`: timing on the same host, information only.
- `analyze.py`: the summary of the gating runs and bars L1 to L4.
- `cpu8.py`: CPU time at 8 threads, base against the branch (information).
- `gating/`: the gating run, made by the A/B helper on a cloud container (its `README.md` has the host and the gate); `run-ab-helper.sh` and `run-gated.sh` are the requests for the VM classes, whose hosts
  did not pass the host-health gate that day.
- `followup/`: why 1080p did not move on the 2.50 GHz host (4 cores): logical CPUs kept busy and scaling against FFmpeg's
  (`busy-1080p.txt`), and the diagnostic build's shares at 1080p.
