# DecodeDecision in assembly: the count and the local timing

`docs/design/h264.md`, "Entropy round: DecodeDecision in assembly", states the rules these were run under
(committed in `3e383e0e`, before the build) and what they conclude. Local evidence, not a bar run.

- **Builds** (`binaries.sha256`): `dist` builds of the `decode` example of `coreav-h264`.
  - `bin-adbase`: `ac315f8c` (`perf/h264-residual-dispatch`, the base).
  - `bin-ad2`: the branch's code commit `200f9a79` (`k`).
- **Host:** a development VM, Intel Xeon @ 2.50 GHz, family 6 model 85, 8 logical CPUs.
- **The count (AD-0):**
  - `count-a81.txt`: callgrind total instructions, 30 pictures of `a81de82106be`, `--discard`. Its regular bins (2.54 M)
    are the decision probe's count (`../cabac-decision-probe/plan.txt`).
  - `count-32.txt`: `cg32.sh`, callgrind of both builds on the 32 training streams at a tenth of the class frame
    counts, total instructions per stream.
- **Identity:** `identity.txt`, `ident.sh`: md5 of the decoded output of both builds on the 32 streams at a tenth of
  the class frame counts, at 1 and 8 threads.
- **Conformance:** `conformance.txt`: `tests/conformance.rs` with `COREAV_H264_THREADS=1,2,8`, default build with
  detected kernels, default build with `COREAV_H264_KERNELS=scalar`, and `--no-default-features`.
- **Timing:** `ab3.py 8 <the 32 streams> bin-adbase bin-ad2` → `ab-k.txt`: one thread pinned to CPU 3, `--discard`,
  class frame counts (sd 1800, 720p 900, 1080p 300, 4k 150), order rotated each round, the child's user plus system
  CPU time; per stream the median of `k` over the median of `base`. `abq.py` → `ab-k-summary.txt`: medians with
  bootstrap 95% intervals, overall, by entropy coder and by class. It reads the CABAC/CAVLC split from the decision
  probe's bin counts (`/tmp/ent/bins.pkl` as run; the CAVLC streams are listed in `../cabac-decision-probe/plan.txt`).
- **Fuzz:** `fuzz.txt`, the last lines of a local run of the `instances` target (nightly, address sanitizer), seeded
  with nine small CABAC conformance streams.
