# Code placement check (information, after the gating run)

The gating run's 4k streams gained more than their counts allow: `ed75c044e944` (0.845), whose
luma samples are 99.9% full-sample copies (`../positions.txt` method), a loop this round does not
change. A `perf` profile with software clock sampling (this VM has no PMU) put the difference in
`predict` itself (11.6% of base's samples, 7.8% of `k`'s), on the same instructions at other
addresses. So, to separate code placement from the kernels, both arms were rebuilt from the gated
worktrees with every function and loop aligned to 64 bytes
(`RUSTFLAGS="-C llvm-args=-align-all-functions=6 -C llvm-args=-align-loops=64"`, otherwise as
gated), output identical on the 32 training streams, and timed with `~/tools/abn.py`: interleaved,
pinned to one core, `--discard`, the class frame counts, the median of 3 rounds per stream, no
FFmpeg. The host gate passed before and after each file's run.

| File | Binaries | Result |
|---|---|---|
| `probe-align-4k.json` | aligned base, aligned k, base, k (the gated binaries); 4k | aligned k over aligned base 0.996 (streams 0.971 to 1.038); k over base 0.979; aligned base over base 0.967 (`ed75c044e944` 0.793) |
| `probe-align-all.json` | aligned base, aligned k; sd, 720p, 1080p | 0.993 [0.990, 0.995]: sd 0.9935, 720p 0.9932, 1080p 0.9910 |

Over all 32 streams, aligned k over aligned base: 0.9930 [0.9892, 0.9956], 25 of 32 faster; the
nine weighted streams 0.9920, the 23 others 0.9942.
