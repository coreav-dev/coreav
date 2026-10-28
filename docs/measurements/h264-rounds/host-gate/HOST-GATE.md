# Host-health gate

Cloud VMs of the same class are not always equally fast: round 13's first gating run (2026-10-23) ran with FFmpeg's own time
up to 2.4 times the reference on single streams, with no sign of it in the guest (steal and cgroup throttling at zero).
Since then every timing call of a gating run is bracketed by a gate:

1. Before the call, `hostgate.py` times FFmpeg 9.0.2 on two fixed streams (`36501d448eaa.0.720p`, 900 frames; and
   `a6af290d9053.0.1080p`, 300 frames), pinned to core 2, three times each, median. Both must be within +-5% of the
   host class's recorded reference. If either is not, wait (a minute) and retry; do not start the call.
2. After the call, the same check. If it fails, the call is void (a host fault): its report and log are kept as
   `*.void<N>`, the call is run again, and both outcomes are recorded in the run's `hostgate.log` (in a container run, its `log.txt`).
3. Reference times are per host class (Intel Xeon family 6 model 106, 2.90 GHz; model 85, 2.50 GHz) and are in the script.
4. A round's A/B (both arms in the same run) may use either VM class or the Compute Engine class below; the class goes on record (earlier rounds also used a cloud container, below). Held-out verdicts and
   FFmpeg-ratio snapshots use one fixed class (2.90 GHz; for rounds whose bars are committed after `docs/design/h264.md`, "The one-thread class of rounds
   whose bars are committed from here on", the Compute Engine class below, with the 2.90 GHz class as information).

## Compute Engine one-thread class

`c4-standard-8`, one thread a core (family 6 model 207). Its references are not in the script: each run measures its own (the two gate streams three times,
five minutes apart, which must agree within 5%; the median is the reference) and checks before and after every call within 5% of them, with the call run again
after a failed after-check (3 attempts, voided files kept) and not started on a failed before-check. `scripts/h264_parity_gcp.py --one-thread-class` does this
(`gate1t.py` is the same check as `hostgate.py`, with the run's references).

## Earlier cloud-container runs

Rounds 13 to 18 and a few later ones ran their A/B in one cloud container when the VM classes' hosts were noisy. Each took a fresh
reference for its host (the two gate streams three times, five minutes apart, stable when within **+-5%** of the median) and
bracketed every call by the gate; the host class is in each round's README. Their runner scripts are not published, and new
rounds use the classes above.
