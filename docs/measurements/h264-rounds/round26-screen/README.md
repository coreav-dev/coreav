# Round 26 screenings: the co-located corners and the macroblock record

Evidence for `docs/design/h264.md`, "Stalls at the macroblock record, moved not removed". Development VM, Intel Xeon
@ 2.90GHz (family 6 model 106). Local screenings, not gated rounds: neither arm reached the bar that would have
queued it. The scripts ran from a local work directory (`~/h264work/r26`), whose paths they name; `ab3.py` and
`abcls.py` are those of `../mb-stalls/`.

Builds: `col-base` = `main` (its code is the same from `056bd25` to `3d60f0b`; the build `dw-4cb168e` of
`../mb-stalls/`); `col-c`, `col-ch` =
`rejected/h264-colocated` at `23d67c8` without and with the hint; `rec-p1`, `rec-p2`, `rec-h` = the record
store fix, the fix with the write hints (`rejected/h264-mb-record` at `aa45cc8`), the hints alone.
`prof-8e70ee5` = `main` at `8e70ee5` built as `dist` with line tables added, `prof-rec-p1` = the
same with the record fix.

- `plan.txt`: the three screenings' bars, written before each run.
- `identity.txt`: decoded-output md5 of every arm against `main`, one and eight threads, all 32 streams (all same).
- `ab-colocated.txt` (plan 26), `ab-record-p1.txt` (26b), `ab-record.txt` (26c): `ab3.py`, 5 rounds, all 32 streams, pinned; ratio to
  `col-base` per stream. `classes.txt`: per class with `abcls.py` (median, bootstrap 95% interval).
- `lines-sd-6f7799768413.txt`: `addr.py` on `prof-8e70ee5`: per source line of our code (folded to the innermost
  inline frame in a CoreAV crate), perf `cpu-clock` time share (3 runs) against callgrind's instruction share
  (`--dump-instr=yes --cache-sim=yes --branch-sim=yes`) and its simulated misses per 1000 instructions. Callgrind
  reports this position-independent binary's addresses as ELF virtual addresses (base 0). Valgrind runs the AVX2
  kernels, so the AVX-512 lines show time and no instructions; read the MB layer's (scalar) lines only.
- `record-p1-shift.txt`: `mbdiff.py`, per line, ms a run, `prof-8e70ee5` against `prof-rec-p1` (3 runs each;
  the totals moved with the host between the two sets, the per-line moves are the reading). `mbline.py` prints
  chosen lines per build.
- `record-misses.txt`: `d1w.py`, the record copy's (`mb.rs:818`) and `store_mb`'s (`inter.rs:569`, its motion row copy) callgrind events split into read and write
  misses, and the record copy's two stores as disassembled from `prof-8e70ee5`.
