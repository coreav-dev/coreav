# PGO round: gating run on the A/B helper's VM

Commit `7a177c62` (`perf/h264-pgo`, the commit that holds `docs/design/pgo.md`); both arms were built from it on this host with rustc 1.99.1: `before` is the plain
`cargo build --locked --profile dist --example decode -p coreav-h264` (`--no-default-features` for the safe mode), `after` is `scripts/pgo_build.py --mode fast|safe` (`pgo-build-*.log`).
`binaries.sha256` (decode-before-fast `f81c8173...`, decode-after-fast `6b89db50...`, decode-before-safe `6bc27ce0...`, decode-after-safe `d7a8ad58...`, and the two profiles). FFmpeg 9.0.2 `0d709a0b...`.
`run-pgo-helper.sh` (derived from the deblock-driver round's script), `analyze.py` of `7a177c62`. The script's last step (copying `analyze.py`) failed on a relative path after the timing
had finished; `analysis.txt` was produced by hand with the same `analyze.py` (`python3 -I analyze.py .` in this directory), and nothing was re-run.

Training input of the profile: the 97 in-scope JVT bitstreams. `inputs/fetch.py` fetched each zip (SHA-256 checked against the manifest `crates/coreav-h264/tests/data/conformance-streams.json`)
and kept only the bitstream the manifest names (`inputs/prune.py` removed the rest of the first 44 extractions, and `fetch.py` does it for the others). The held-out streams were extracted by
`inputs/extract.py` of `docs/measurements/h264-n2-heldout/tools/` and match `h264-n2-heldout/es.json` (64 of 64; `provenance.txt`).

**Host class: the helper's VM, Intel Xeon Processor @ 2.90 GHz (family 6 model 106), AVX-512 present.** Identity (`identity.txt`): 258 of 258 same: for the fast and the safe mode, the 32 training streams
with `after` at 1 and 8 threads against `before` at 1, and the 97 conformance bitstreams at 1 thread (output hash and exit status). Sixteen calls between gate checks (`g`: fast, `--pin 2 --rounds 5`;
`t8`: fast, `--threads 8` unpinned, 10 rounds; `gs`: safe, as `g`; `h`: fast, as `g`, the 64 held-out streams; one call per class), `hostgate.log`: 33 passes and 7 failed checks (four before-checks
of `t8.sd` and `t8.1080p` and two of `gs.sd`, each waited and passed; **one void**: the after-check of `gs.sd`, run again and passed, files kept as `gs.sd.*.void1`), **REBASE count 0**. Spend: none (local VM).

Result (`analysis.txt`): `PG-1 False, PG-2 False, PG-3 False, PG-4 True, PG-5 True, PG-6 True` (PG-7 is read from `identity.txt`: all same). After over before, median per-stream ratio (95% bootstrap interval):

| Call | discard, all | write, all | per class, discard (sd, 720p, 1080p, 4k) |
|---|---|---|---|
| `g` fast, 1 thread, 32 training | 0.9929 [0.9796, 1.0083] (19/32 faster) | 1.0001 [0.9817, 1.0147] | 0.9852, 0.9891, **1.0116**, 0.9841 |
| `t8` fast, 8 threads | 0.9811 [0.9687, 0.9932] (24/32) | 0.9870 [0.9821, 1.0017] | 0.9733, 0.9805, 0.9982, 0.9693 |
| `gs` safe, 1 thread | 0.9804 [0.9613, 0.9941] (29/32) | 0.9915 [0.9746, 0.9993] | 0.9765, 0.9909, 0.9870, 0.9539 |
| `h` fast, 1 thread, 64 held-out | 0.9919 [0.9838, 1.0045] (34/64) | 1.0037 [0.9902, 1.0107] | 1.0095, 0.9884, 0.9868, 0.9905 |

PG-1 needs at most 0.990 (read 0.9929); PG-3 needs at most 1.005 per class in both modes (`g`: 1080p discard 1.0116, 720p write 1.0101).
Against FFmpeg 9.0.2 (median per-stream ratio, `before` then `after`; share where ours is faster): `g` discard 1.125 then 1.124 (8/32 then 9/32), write 1.083 then 1.094 (8/32, 10/32);
`t8` discard 1.022 then 1.004 (13/32, 16/32), write 1.028 then 1.018; `gs` discard 1.188 then 1.149 (6/32, 7/32); `h` discard 1.102 then 1.126 (13/64, 15/64), write 1.083 then 1.096.
