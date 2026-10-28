# Kernel round: boundary strengths in vector registers

`docs/design/h264.md`, "Kernel round: boundary strengths in vector registers", states the probe that motivated the
round (`../bs-probe/`), the design and bars BS-1 to BS-5, fixed before any gating timing.

- `base` = `main` at `bb7255cd`; `k` = `193051ba`, the branch's code commit.
- `run-ab-helper.sh BASE K` is the one-thread gating run on the helper's VM (family 6 model 106), into `gating/`.
  The harness runs with `--shuffle-seed 20261026 --null-arm base --gate-stream 41e324ebb708.0.sd.h264 --gate-every
  4`, one call a class pinned to core 2 at twenty rounds, each between two host gates.
- `run-gcp.sh BASE K OUT` is the eight-thread run on a new `c4-standard-16` VM (`scripts/h264_parity_gcp.py`; "Where
  the eight-thread arms run"), into `gating-gcp/`, at most 2.5 hours and 5 USD.
- `analyze.py DIR` prints the bars from `g.*.json` and, with them, `t8.*.json` (BS-4 pending without them).
- Local timing, information (one thread pinned, the 32 streams): before an earlier change, the round over `main` with
  `store_mb` inlined, five rounds, discard 0.9835 [0.9775, 0.9900], 28 of 32 faster; on an earlier change, `base` against `k`
  (builds of the same trees), six rounds, discard 0.9834 [0.9729, 0.9926], 28 of 32 faster (sd 0.9663, 720p 0.9928,
  1080p 0.9998, 4k 0.9732), write 0.9807 [0.9748, 0.9850], 31 of 32. The first build with 512-bit registers:
  1.0141 [1.0062, 1.0183] over the same code without the kernel (design note).
- Output (local, `k` against `base`): identical on all 32 streams at 1 and 8 threads with the detected kernels, and
  at 1 thread with AVX2 and with scalar kernels. The gating run's `identity.txt` repeats it at 1 and 8 threads.
- `conformance.txt`: 97 of 97 at 1, 2 and 8 threads, detected and scalar kernels, on `193051ba`. `fuzz.txt`: `strengths` (the
  AVX-512 kernel and `Auto` against the reference on arbitrary motion), 600 s, clean.
- Gating (one run an arm, BS-1 to BS-5 met): `gating/` (one thread, helper VM, with its README and the void first
  4k call) and `gating-gcp/` (eight threads), from branch `measured/h264-bs-kernel-gating`; results in the design
  note's "Result".
- After the rebase onto `main` at `75121732` (bundle 2 included): output identical to that `main`'s on the 32 streams
  (1 and 8 threads detected, 1 thread AVX2) and conformance 97 of 97 at 1, 2 and 8 threads, detected and scalar
  (`rebase/`: `id.<threads>.<kernels>.txt`, stream, `main` md5, branch md5; the conformance reports' summaries).
- The disassembly of `k`'s dist build has no `zmm` register (`objdump -d decode | grep -c zmm` is 0), nor has
  `base`'s.
