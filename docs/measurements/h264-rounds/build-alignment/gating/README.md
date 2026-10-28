# Gating run: the build-alignment round

`run-ab-helper.sh 32bbbff 433ca30` (the script beside `analyze.py` one directory up), on this
development VM: Intel Xeon @ 2.90 GHz, family 6 model 106, AVX-512 (`provenance.txt`). `base` = `main`
at `32bbbff`, built as users have had it; `k` = `433ca30`, the same tree plus `.cargo/config.toml`
(every function and loop on a 64-byte line). Both built alike (`dist`, `RUSTFLAGS` unset) from one
worktree each. `g.*` is one thread pinned to core 2, `t8.*` eight threads unpinned, five rounds, the
class frame counts. Output identical on 32 of 32 streams for `k` at 1 and 8 threads against `base`
at 1 (`identity.txt`); the host gate passed before and after each of the eight calls, none void
(`hostgate.log`); `analysis.txt` is `analyze.py`'s output, which reproduces from the `*.json`
reports. AL-4 was not met (8 threads, write, 1.00503 against at most 1.005); the round is rejected
(docs/design/h264.md, "Build round: code alignment"). `../k.patch` is the change that was timed,
with the wheel check.
