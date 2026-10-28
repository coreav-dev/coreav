# Luma interpolation round: counts and timing

Instruction counts behind `docs/design/h264.md`, "Kernel round: luma
interpolation by position". Callgrind counts, and the gating run's
timing.

- `calls.txt`: the decoder's kernel calls on the 32 training streams at
  their class frame counts (sd 1,800, 720p 900, 1080p 300, 4k 150),
  counted by `main` at b66e452f with `tools/calls.patch` applied (a
  counter per shape in `Auto`, printed by `examples/decode` at exit).
  Columns: stream, kind (0 `luma_mc`, 1 `luma_mc_avg`, 2 `combine`), w,
  h, xFrac, yFrac (for `combine`, xFrac is the mode: 0 copy, 1 average,
  2 one list weighted, 3 two lists weighted), calls.
- `shapes.txt`: instructions a call of `Auto::luma_mc` or
  `Auto::luma_mc_avg` per shape and position, for `main`, candidate A and
  candidate C, from `tools/count.sh` (the harness `tools/lmccount.rs`,
  `lto = "fat"`, one codegen unit). About 135 instructions of each count
  are the harness's own.
- `instructions.txt`: the whole decode of four streams under callgrind,
  `examples/decode --discard`, one thread, `dist` profile with line
  tables, for `main`, A and C, each built from its commit; and the old
  kernel's functions on `main`, with their simulated indirect
  mispredicts.
- `shapes-first-version.txt`: the same count for the first version of
  A, whose helpers were closures (design note, point 5).
- `tools/weigh.py shapes.txt calls.txt COLUMN`: the per-shape counts
  weighted by the calls (column 0 `main` 100.0 G, 1 A 33.0 G, 2 C
  28.9 G);
  `tools/fnsum.py`: per-function self and inclusive counts of a
  callgrind file.

The gating run (bars LM-1 to LM-4 and LM-C), on the VM that reports the
2.90 GHz class:

- `run.sh`: the run, three decoders (`main` at b66e452f as the base, A
  at b96e94ce, C at 3d8d6b96) built alike from one tree each, the
  outputs checked, then one `scripts/h264_parity.py --pin 2 --rounds 5`
  call per class with FFmpeg 9.0.2 as the reference command;
- `provenance.txt`: the binaries' SHA-256, the compiler, the CPU, FFmpeg
  and the harness;
- `identity.txt`: each stream's output MD5 for the base at one thread
  and A and C at one and eight;
- `g.<class>.json`: the harness's per-stream times; `g.summary.json`:
  the paired medians, intervals and ratios to FFmpeg from `analyze.py`.
