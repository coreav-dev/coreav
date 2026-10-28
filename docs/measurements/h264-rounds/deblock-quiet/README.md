# Deblocking round: macroblocks below the thresholds

Design, bars and results: `docs/design/h264.md`, "Deblocking round: macroblocks below the thresholds". The local host
is a development VM (Intel Xeon @ 2.90 GHz); its timings are directional. Builds: `cargo build --locked --profile dist
--example decode -p coreav-h264`, `RUSTFLAGS` unset.

| name in the files | build |
|---|---|
| `base`, `bin-base` | `main` at `dd85b381` |
| `k`, `bin-k` | the branch's code commit `4075f864` |

- `count.diff`, `run/deblock-counts.txt`: a counting build on `main` (not for merge). Per stream, at the class frame
  counts, it counts the macroblocks the filter visits (`mbs`, 0 where deblocking is off), the share whose QP and whose
  filtered neighbours' QPs are below the first live QP (`quiet`), the share whose strengths are all 0 (`zero`), and
  the share that are quiet and have a strength above 0 (`quiet&nonzero`), which only this round passes over.
- `cgfull.sh STREAMS A B`, `run/instructions.txt`: callgrind instruction totals, one thread, `--discard`, at the class
  frame counts. The target streams T of the bars are those with 1.5% or more fewer instructions.
- `ident.sh A B`, `run/identity.txt`: md5 of the output of the 32 training streams at the class frame counts, 1 and 8
  threads.
- `run/identity-nofast.txt`: the same for builds with `--no-default-features` (without `fast`).
- `run/conformance.txt`: `scripts/h264_conformance.py --threads 1,2,8` on `k`, with detected kernels, scalar kernels,
  and `--mode safe` (without `fast`).
- `ab3.py`, `abset.py`: the local timing (eight rounds, one thread pinned, discard, the 32 streams) and its medians
  over T, the other 22 (R), all 32 and each class.
- `analyze.py DIR`: the bars QZ-1 to QZ-6 from the gating calls' reports.
