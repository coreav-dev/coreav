# Profile-guided optimisation of the decoder build

Status: Proposed.

The rounds in `docs/design/h264.md` change the decoder's code one area at
a time. Profile-guided optimisation (PGO) changes how the compiler lays
out and inlines all of it, from branch counts gathered on a run: a build
step that reaches every stage at once, with no change to what the decoder
computes. This note covers how the build is made, what trains it, and the
bars it must meet before any shipped artifact uses it.

## Design

1. **What is built.** The `dist` build of the decode example
   (`cargo build --locked --profile dist --example decode -p coreav-h264`,
   the binary every round measures), with LLVM's instrumentation PGO
   through `rustc`'s own flags. `scripts/pgo_build.py` (on `rejected/h264-pgo`
   at `7a177c62`) makes it in three
   steps from one tree and one toolchain: an instrumented build
   (`-Cprofile-generate`), a training run, and the optimised build
   (`-Cprofile-use`) from the merged profile. Each mode has its own
   profile: `--mode fast` (the default features) and `--mode safe`
   (`--no-default-features`, no `fast` module).
2. **What trains it.** The 97 in-scope streams of the JVT conformance
   suite, listed in `crates/coreav-h264/tests/data/conformance-streams.json`
   (the list `scripts/h264_conformance.py` reads), each decoded with
   `--discard` at one and at eight threads. The input is public, fixed by
   that list and already fetched by the conformance job. It never includes
   the 32 training streams or a held-out set: a profile trained on the
   streams the bars are read on would fit the measurement, not the decoder.
3. **What it needs.** `llvm-profdata` from the `llvm-tools` component of
   the same Rust toolchain, so that the profile format matches `rustc`'s
   LLVM. It is a toolchain component, not a crate, so `deny.toml` is
   unaffected.
4. **Reproducible inputs, not bytes.** The same tree, toolchain and list
   give the same training input. The eight-thread runs' counts vary with
   scheduling, so two builds need not be byte-identical. PGO changes no
   semantics, and the output is checked to be identical to the plain
   build's (PG-7).

Rejected:

- **`cargo-pgo`.** It wraps the same `rustc` flags and `llvm-profdata`
  merge. It is one more tool to install for nothing the script needs.
- **Synthetic training streams** (`local/synth.sh`: six x264 streams from
  FFmpeg's test sources, 720x480 interlaced to 3840x2160, Baseline to
  High) alone or with the conformance suite. Locally they read 0.9873 and
  0.9868 against 0.9842 for the conformance suite alone in the same run
  (`local/local.txt`), so they add nothing.
- **A committed profile.** It goes stale with every change to the code it
  describes, so the build regenerates it instead.
- **BOLT** (post-link layout). Not evaluated here. Neither the Rust
  toolchain nor the distribution provides `llvm-bolt`, and these VMs have
  no branch-stack sampling (`perf record -b` is not supported), so only
  BOLT's instrumentation mode would be open to it. It is a later probe.

## Local probe

As information (`docs/measurements/pgo/local/`; a development VM of the
2.90 GHz class; the PGO builds from the conformance profile). CPU time, one
thread pinned (`luma8x8/ab.py`), all 32 training streams, five interleaved
rounds, median per-stream ratio over the plain build of `main` at
`23b45d4c` (whose crates are `3d60f0bf`'s):

| Run | Mode | All 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|---|
| a | fast | 0.9915 [0.979, 0.999], 24/32 | 0.9772 | 0.9926 | 0.9957 | 0.9795 |
| a | safe | 0.9843 [0.974, 0.988], 29/32 | 0.9774 | 0.9864 | 0.9849 | 0.9770 |
| b | fast | 0.9842 [0.971, 0.995], 25/32 | 0.9721 | 0.9898 | 0.9960 | 0.9699 |

At eight threads, unpinned (`local/ab8.py`, five rounds), fast mode reads
0.9807 in CPU time and 0.9998 in wall time; safe mode reads 0.9726 and
0.9743. The eight-thread intervals span about 0.95 to 1.02 on this
eight-vCPU VM.

## Shipping, if the bars pass

- The release builds of the x86-64 Linux artifacts (the wheels and the npm
  packages' binaries) gain the same three steps in a later change. That
  change collects the profile by running the shipped binary itself on the
  conformance streams, so that the profile describes the code it
  optimises. This round wrote the script and measured the lever on the
  binary the rounds measure.
- Cost: on the probe's VM each `dist` build takes about a minute and the
  training run about a minute, so three steps add about three minutes per
  artifact and mode. The conformance zips come from the cache the
  conformance job already keeps.
- Other targets (aarch64, macOS, Windows) stay plain until they are
  measured on their own hardware, because a profile is collected by
  running on the target. `cargo install` builds stay plain.

## Bars

Fixed before the gating run, by the rounds' method on the 2.90 GHz class
(`scripts/h264_parity.py --require-cpu 'Intel(R) Xeon(R) Processor @
2.90GHz'`, the host gate before and after each call, one call per class at
its frame count), with FFmpeg 9.0.2 (BtbN `n9.0.2-17-g2a571b6068`) as the
reference of every call.

Both arms are built on the run's host with one toolchain, from the commit
that holds this note:

- `before`: the plain `cargo build --locked --profile dist --example decode
  -p coreav-h264`, with `--no-default-features` for the safe mode;
- `after`: `scripts/pgo_build.py --mode fast` (or `--mode safe`).

The calls are:

- `g`: fast mode, `--pin 2 --rounds 5`, the 32 training streams;
- `t8`: fast mode, `--threads 8`, unpinned, ten rounds;
- `gs`: safe mode, `--pin 2 --rounds 5`;
- `h`: fast mode, `--pin 2 --rounds 5`, the 64 streams of the held-out
  draw (`docs/measurements/h264-n2-heldout/list.json`) at its frame counts.

`docs/measurements/pgo/analyze.py` reads them (`python3 -I analyze.py
DIR`). Each bar is a median of per-stream ratios `after` over `before`,
with its 95% bootstrap interval.

| Bar | Quantity | Threshold |
|---|---|---|
| PG-1 | `g`, discard, all 32 | at most **0.990**, upper end of the interval below 1.000 |
| PG-2 | `g`, write, all 32 | at most **0.995** |
| PG-3 | `g`, each class, discard and write | at most **1.005** in each |
| PG-4 | `t8`, discard, all 32; and each class | at most **1.000**; at most **1.010** in each |
| PG-5 | `gs`, discard, all 32 | at most **0.995** |
| PG-6 | `h`, discard, all 64 | at most **0.995**, upper end below 1.005 |
| PG-7 | output | each `after` build byte-identical to its `before` build on the 32 training streams at 1 and 8 threads, and on the 97 conformance streams at 1 thread (the plain builds pass conformance in CI) |

PG-1 asks for the gain the probe showed (0.9915 and 0.9842 in two local
runs), PG-6 that it holds on streams the profile and the earlier rounds
never saw, PG-5 that the safe mode gains too, and PG-2 to PG-4 that no
output mode, class or thread count loses. PGO is adopted for the release
builds only if PG-1 to PG-7 pass. Otherwise this note records the result,
and the script stays for a later toolchain or training input.

## Result: not adopted, by PG-1, PG-2 and PG-3

`docs/measurements/pgo/gating/` (`README.md`, `analysis.txt`, the reports, `identity.txt`, `hostgate.log`,
`provenance.txt`, the two build logs; branch `measured/h264-pgo` `52ce08dd`): one run on a development VM of
the bar class (Intel Xeon @ 2.90 GHz, family 6, model 106), both arms built there with rustc 1.99.1 from
`7a177c62`, the commit that held this note with its bars. 33 gate checks passed and 7 failed; six of
those waited and then passed, and one voided its call (`gs.sd`), which was run again and passed, the voided
files kept as `*.void1`. `analyze.py` reports no failed or missing call. The run's last step, copying
`analyze.py`, failed on a relative path after the timing had finished; `analysis.txt` was produced with the
same `analyze.py`, which was then copied in by hand; nothing was re-run.

| Bar | Result | Threshold |
|---|---|---|
| PG-1, `g`, discard | **0.9929 [0.9796, 1.0083]**, 19 of 32 faster: missed | at most 0.990, upper end below 1.000 |
| PG-2, `g`, write | **1.0001 [0.9817, 1.0147]**, 16 of 32: missed | at most 0.995 |
| PG-3, `g`, per class | discard sd 0.9852, 720p 0.9891, **1080p 1.0116** (1 of 8 faster), 4k 0.9841; write sd 0.9831, **720p 1.0101**, 1080p 1.0029, 4k 0.9862: missed | at most 1.005 in each |
| PG-4, `t8`, discard | all 32 0.9811 [0.9687, 0.9932], 24 of 32; sd 0.9733, 720p 0.9805, 1080p 0.9982, 4k 0.9693: met | at most 1.000; at most 1.010 in each |
| PG-5, `gs`, discard | 0.9804 [0.9613, 0.9941], 29 of 32: met | at most 0.995 |
| PG-6, `h`, discard | 0.9919 [0.9838, 1.0045], 34 of 64: met | at most 0.995, upper end below 1.005 |
| PG-7, output | identical, 258 of 258 (each mode: 32 training streams at 1 and 8 threads, 97 conformance bitstreams at 1 thread; output hash and exit status): met | |

The release builds stay plain, and "Shipping" above does not happen. In the mode that ships, fast, at one
thread, PGO moved the training set by 0.7% at the median (19 of 32 faster; write 16 of 32), with the 1080p
class slower on 7 of 8 streams and write mode unchanged (1.0001); on the held-out streams discard reads
0.9919 and write 1.0037. Where it did gain is the safe mode (2.0%, 29 of 32 faster; 4.6% at 4k, 8 of 8)
and eight threads (1.9%; write 0.9870). A likely reading, not measured: the fast mode's hot paths are the
hand-written kernels and the `fast` module, whose layout the compiler's profile changes little, while the
safe mode's bounds checks and the threaded scheduling are ordinary compiled branches that a profile can
lay out.

Against FFmpeg 9.0.2 on that host (median per-stream ratio, `before` then `after`, share of streams where
CoreAV is faster; information): `g` discard 1.125 (8 of 32) and 1.124 (9), write 1.083 (8) and 1.094
(10); `t8` discard 1.022 (13) and 1.004 (16), write 1.028 (14) and 1.018 (14); `gs` discard 1.188 (6)
and 1.149 (7); `h` discard 1.102 (13 of 64) and 1.126 (15), write 1.083 (19) and 1.096 (18).

The build script does not merge, unlike the plan in "Bars": it is on `rejected/h264-pgo` at `7a177c62`, the commit both arms were
built from. A later toolchain or training input can be measured with it as a new round with its own bars;
these arms are not measured again.
