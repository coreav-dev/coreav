# Round 27: where the MB layer's stores go, and a probe of one motion entry per macroblock

Evidence for `docs/design/h264.md`, "Where the stores go, and a probe that could not bound them". Development VM,
Intel Xeon @ 2.90GHz (family 6 model 106). The scripts ran from a local work directory (`~/h264work/r26`, `r27`,
`r28`), whose paths they name; `addr.py`, `mbline.py`, `ab3.py` and `abcls.py` are those of `../round26-screen/` and
`../mb-stalls/`. The profiled build is `prof-8e70ee5` of `../round26-screen/` (`main`'s code with line tables).

- `lines-<stream>.txt`: `addr.py` per source line (perf `cpu-clock` time share against callgrind's instruction share
  and simulated misses per 1000 instructions) for sd `6f7799768413` (CAVLC), 720p `1d4b1775d78a`, "1080p"
  `a81de82106be` (1560x720) and 4k `3b1b441e23ae` (CABAC).
- `chain-<stream>.txt`: `addr.py` limited to the decision chain's lines (`cabac.rs` 194 to 215) on the three CABAC
  streams; `chain-sum.txt` sums each.
- `stores-<class>.txt`: `stores.py` on the same four streams: stores, bytes and simulated L1/LL write misses per
  macroblock by source line. `d1w.py` splits one line's callgrind events into read and write misses.
- `plan-lps.txt`, `ab-lps-pair.txt`, `classes-lps-pair.txt`, `identity-lps-pair.txt`: the LPS pair table
  (`rejected/h264-cabac-lps-pair`, build `r28-lps`): bars, `ab3.py` 6 rounds all 32, `abcls.py`, and decoded-output
  md5 against `r28-base` at one and eight threads (`ident.sh`).
- `plan.txt`: the probe's arms and bars, written before the run.
- `probe.diff`: the probe (not for merging): `store_mb`'s motion rows 1 to 3 to a scratch array, output wrong.
  Builds: `r28-base` = `main` at `23b45d4` (md5 the same as `col-base` of `../round26-screen/`), `r28-probe` = that
  plus `probe.diff`; both `dist`, `--example decode`, separate target directories.
- `ab-probe.txt`: `ab3.py`, 6 rounds, all 32 streams, one thread, `--discard`, pinned; ratio to `r28-base` per stream.
  `classes-probe.txt`: `abcls.py` (median, bootstrap 95% interval, wins).
- `probe-instructions.txt`: cachegrind (`--cache-sim=no`), 180 pictures of `1d4b1775d78a`, both builds: total and
  top functions by instructions, showing that the probe changed the decode's work.

Spend: none (local VM only).
