# Band height at eight threads: the sweep plan (written before the sweep)

## Why

At eight threads every reference picture is cut into bands, and each band after the first copies a top apron: 23
luma rows and 9 chroma rows from the bands above, plus 4 luma rows, 2 chroma rows and a row of motion when decoding
enters it (`Canvas::enter`, `Canvas::refresh_top`, `copy_top` in `picture.rs`). The band height is about a
sixteenth of the picture, a power of two between 1 and 8 macroblock rows (design note, threading rule 4): 1 for sd,
2 for 720p, 4 for 1080p, 8 for 4K, so the apron is 144%, 72%, 36% and 18% of the luma a band owns. Callgrind put
`copy_top` at 12.0, 5.6, 3.3 and 2.2% of the one-thread instructions on one stream a class
(`band-views/plan-probe.md`). Copying the apron twice cost 1.0077 [0.9901, 1.0188] of the eight-thread wall time
over the 32 training streams, sd 0.9979, 720p 1.0066, 1080p 1.0156, 4k 1.0039 (`band-views/probe.txt`): the copy is
worth under 1% overall, so a faster copy alone would not be measurable. The band height decides how much is copied,
and also how soon a band is published for the pictures that predict from it. The rule was set by reasoning, never
measured. This sweep measures it.

## Arms (one tree each, `main` at `16c3e239`, built alike: `cargo build --locked --profile dist --example decode -p coreav-h264`)

- `m`: `main`.
- `x2`: `Geometry::band_mbs_for` returns twice `main`'s height, clamped to 2 to 16: 2 rows for sd, 4 for 720p, 8 for
  1080p, 16 for 4K (an eighth of the picture).
- `x4`: four times, clamped to 4 to 32: 4, 8, 16 and 32 rows (a quarter).

The only change is that function's result; pictures that are not references stay one band, and one thread stays
one band. Each probe's output is checked byte-identical to `m` on the 32 training streams at 8 threads before the
sweep.

## Run

`ab8.py 10` (`~/h264work/r32/ab8.py`, copied here as `ab8.py`): eight threads, unpinned, `--discard`, class frame
counts (sd 1800, 720p 900, 1080p 300, 4k 150), the 32 training streams, arms `m`, `x2`, `x4` rotated each round, one
warm-up round; per stream the median of 10 rounds over `m`'s. Wall time decides; CPU time is information. The host
is a development VM of the bar class (Intel Xeon @ 2.90 GHz, family 6, model 106, 8 logical CPUs). A host fault (a
crashed or suspended run) voids the sweep; nothing else re-runs it.

## Rule (fixed now)

- SW-1: for each class, the arm with the lowest class median (wall, over `m`) is chosen if that median is at most
  **0.985**; otherwise the class keeps `m`'s height.
- SW-2: if a class chooses `x2` or `x4`, a round is built that sets the band height by the classes' choice, written
  as a rule on the picture's height in macroblock rows. That round has its own bars, committed before its timing,
  on the training streams and on the held-out draw (`docs/measurements/h264-n2-heldout/list.json`), whose streams
  this sweep does not see.
- SW-3: if no class chooses another height, no round is built from the band height; the result is recorded here.

## Result

`sweep.txt` and `sweep.json` (per stream, every round's wall and CPU seconds), one run on a development VM of the bar
class (Intel Xeon @ 2.90 GHz, model 106, 8 logical CPUs), no host fault. `x2` is probe branch `probe/r34-bh-x2`,
`x4` is `probe/r34-bh-x4`; both decode byte-identically to `m` on the 32 streams at 1 and 8 threads
(`identity-x2.txt`, `identity-x4.txt`).

| arm | wall, all 32 | sd | 720p | 1080p | 4k | CPU, all 32 |
|---|---|---|---|---|---|---|
| `x2` | 1.0069 [0.9773, 1.0571], 14 of 32 faster | 1.0812 | **0.9724** | 1.0521 | **0.9750** | 0.9541 |
| `x4` | 1.0982 [1.0634, 1.2127], 7 of 32 | 1.2835 | 1.0822 | 1.1141 | 1.0634 | 0.9381 |

By SW-1, 720p and 4k choose `x2` (4 and 16 macroblock rows); sd and 1080p keep `m`'s 1 and 4. The CPU time falls with
the apron in every class but 1080p at `x2`; the wall time rises where a band publishes too late for the pictures
that wait on it, most at sd and with `x4`. By SW-2 a round is built.

### The rule on height, and what is built

SW-2 assumed the classes are cut by height. They are not (`sizes.txt`, from `ffprobe`): pictures 1080 rows high are
in the 1080p class (which kept `m`) and in the 4k class (which chose `x2`), and the classes overlap in height
elsewhere too. No rule on the height reproduces the classes' choices, so the rule is read from the same run by the
band height `m` uses, which is set by the height. This reading was made after the run, from its data; it is
exploratory, and what it selects is checked only by a later run with bars of its own.

| `m`'s band height (picture height, macroblock rows) | streams | `x2` wall, per stream | median |
|---|---|---|---|
| 1 (below 32) | 10 | 1.253 1.094 1.081 1.121 1.007 0.988 1.007 1.003 1.081 0.927 | 1.044 |
| 2 (32 to 63) | 7 | 0.920 1.041 0.968 0.976 0.921 0.909 1.074 | **0.968** |
| 4 (64 to 127) | 13 | 1.057 0.972 0.948 1.025 1.074 1.047 0.979 1.194 1.077 1.130 0.937 0.967 0.917 | 1.025 |
| 8 (128 and more) | 2 | 0.978 1.045 | 1.012 |

Only the pictures of 32 to 63 rows gain from twice the height (4 rows rather than 2). That change touches 7 of the
32 training streams, so its effect on a median over 32 is small: it is not a round alone ("Small gains") but item G
of bundle 2 (`perf/h264-bundle2-g`, `docs/measurements/h264-rounds/bundle2/plan.md`), whose run decides it with the
bundle's bars. The calls here last 0.06 to 1.7 s, the length at which an earlier change's spread rule voids eight-thread calls;
this run has no spread rule, and its per-stream values are medians of 10 rounds.
