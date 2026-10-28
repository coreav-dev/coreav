# Gating run of the CABAC macroblock start round (bars CM-1 to CM-7), one-thread calls

A summary of the run; its raw output is kept with the private measurement records. `base` is `main` before the
round (`8b71d3d5` in `plan.txt`; its H.264 crate is the one in the public release), `k` the round's code
(`5950bef3`), which is not in `main`.

**Method.** `scripts/h264_parity_gcp.py --one-thread-class` of `main`'s harness: the Compute Engine `c4-standard-8`, one
thread a core, Xeon Platinum 8581C, `--pin 2`, null arm `base`, seed 20261026 (the wrapper's default), and the
host-health gate with the class's own references. Two runs at once: the 32 training streams (20 rounds) and the 64
held-out streams (5 rounds). Both binaries were built from the two commits (`dist`, `--locked`) on the machine that
started the runs. Each measure is the median per-stream ratio `k` over `base`, with its 95% bootstrap interval over
streams.

**Output (CM-7).** `k`, and `k` built without `fast`, byte-identical to `base` on the 32 training streams at 1 and
8 threads (32 of 32). Conformance is the round's own (`../conformance.txt`).

**Host faults: none.** Neither run was void; the three gate references agreed and all 8 checks of each run passed; no
call was run again; no stream was unresolved. Largest null deviation of a stream: 1.3% (training), 2.7% (held-out),
against the 4% bar; every round's spread was under 1.50. One start-up fault, before either run: the first launch of
the two runs gave both VMs the same name (they started in the same second), and the second creation, refused, deleted
the first VM, so neither ran. Both were started again 20 seconds apart. `scripts/gcp_vm_job.py` now adds a random
suffix to the name.

## Result: CM-1 missed; CM-2 to CM-5 met; CM-6 not run

| | measure | result | bar |
|---|---|---|---|
| CM-1 | one thread, discard, the 26 CABAC streams | **0.9970** [0.9946, 0.9987], 21 of 26 faster | at most 0.996, upper end below 1.000: **missed** |
| CM-2 | one thread, all 32, discard; write | 0.9972 [0.9954, 0.9996]; 0.9976 [0.9965, 1.0008] | met |
| CM-3 | one thread, sd, 720p, 1080p, 4k | discard 0.9970, 0.9996, 0.9996, 0.9797; write 0.9983, 0.9993, 1.0009, 0.9878 | met |
| CM-4 | one thread, the six CAVLC streams, discard; write | 1.0001 [0.9971, 1.0023]; 0.9994 [0.9968, 1.0022] | met |
| CM-5 | one thread, discard, the 64 held-out streams | 0.9978 [0.9946, 1.0020], 36 of 64 faster | met |
| CM-6 | eight threads | not run: it runs only if CM-1 to CM-5 pass | |

Information: the ten training streams whose still-skip share is 40% or more, discard 0.9797 [0.9686, 0.9971], 10 of
10 faster, write 0.9891; held-out, write 0.9952 [0.9902, 0.9978]; the 15 held-out CAVLC streams, discard 1.0004,
write 1.0020.

Against FFmpeg 9.0.2 (median per-stream ratio, one thread, this class, `base` then `k`): training, discard 0.959
(22 of 32 faster) then 0.956 (23 of 32), write 0.933 then 0.931; the CABAC 26, discard 0.973 (18 of 26) then 0.970
(19 of 26); held-out, discard 0.952 (43 of 64) then 0.958 (46 of 64), write 0.946 then 0.950.
