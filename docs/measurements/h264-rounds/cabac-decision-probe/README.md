# Entropy decoding's instruction excess by syntax element, and a decision probe

Diagnosis and a probe; nothing merges from it. `docs/design/h264.md`, "Entropy decoding: the instruction excess by
syntax element", states what it concludes.

## Breakdown (`breakdown/`)

- **CoreAV:** `main` at `b3664291`, the same crates as `d13b91d0`. The binary is `prof-b3664291` from
  `../pmu-profile/`: `dist`, line tables only.
- **FFmpeg:** a 9.0.2 release build from earlier work, `ffmpeg_g` (sha256 `8d7d112a…`), with its debug information
  removed by `objcopy --strip-debug` before any profiling. The result, `ffsym` (sha256 `09014ae8…`), carries the
  symbol table and no line tables. This VM holds the two binaries only, with no FFmpeg source.
- **Runs:** callgrind of both decoders on the 32 training streams at a tenth of the class frame counts (sd 180, 720p
  90, 1080p 30, 4k 15), `--discard` against `-f null` (`run_cg.sh`).
  - CoreAV is recorded at instruction level. `cg_core.py` resolves each address's inline chain with
    `llvm-addr2line --inlines` on CoreAV's own line tables.
  - FFmpeg is recorded at function level only. `cg_ff.py` reads each function's own instructions, and the calls and
    inclusive cost of `get_cabac`/`get_cabac_noinline` per calling function, from the callgrind file. Names come from
    the symbol table; no source line was produced or read.
- **Stages and categories:**
  - A CoreAV instruction's stage is `../profile-6c9e240/classify.py`'s `core_stage`. Its category is the first match
    of `report.py`'s `RULES`, searched from the innermost frame outward. "Engine" means the innermost frame is
    DecodeDecision, DecodeBypass, the refill or the load.
  - An FFmpeg function's stage is `../pmu-profile/stages_pmu.py`'s `FF_RULES`, and its group is `FFC`.
  - FFmpeg's symbol table has no separate functions for motion vector prediction or most neighbour work: they are
    inlined into `ff_h264_decode_mb_cabac`. So its MB header and MB layer are compared together.
- **Per macroblock:** each decoder's instructions are divided by the macroblocks it decoded (`mbcount.py` →
  `mbs.json`, from callgrind's call counts). For CoreAV that is the calls of `start_mb`'s out-of-line instances. For
  FFmpeg it is the calls of `ff_h264_decode_mb_cabac` and `ff_h264_decode_mb_cavlc`. Both decoders decode past the N
  pictures they are asked to output: CoreAV by its reorder delay (1.00 to 1.73 times the nominal count), FFmpeg also
  by its stream probing (1.03 to 2.00). The probing decodes the stream's first pictures a second time, so FFmpeg's
  average counts those pictures twice, and CoreAV's extra pictures are later ones. This breakdown's +1,377 agrees with
  the PMU profile's +1,355 at full frame counts, so the bias is small. One stream, `6e90422a670a`, gives CoreAV a
  count that is not a whole number of pictures (46.25 against FFmpeg's 45). That moves no median.
- **Bins:** exact counts per macroblock from a counting build of `main`, local only (`count-bins.diff`,
  `bins/*.txt`), by context index and by the residual loops' own counters. Bins depend on the stream, not the decoder.
- **Results:** `final.py` → `final.txt` (groups, bins, per-bin figures); `fnlist.py` → `fnlist.txt` (functions).
  `byfn_pmu.py` → `pmu-byfn.txt` gives the same split from the sampled PMU run of `../pmu-profile/`, as a cross-check.
- **Disassembly:** `decision-asm.txt`, CoreAV's own significance-map loop in `bin-base`, `bin-v2` and `bin-v6`. In
  `bin-base` the inlined DecodeDecision is 40 instructions, from the context index's `lea` at `4eff4` to the refill
  test's `ja` at `4f09a`.
- **Reproducing:** the scripts read the callgrind outputs and `bins/` from `/tmp/ent` as they were run. The inputs are
  `raw.tar.gz`, `bins/*.txt` and `mbs.json`.
- **Raw:** the callgrind outputs are `raw.tar.gz` on branch `measured/h264-cabac-decision-probe`.

## Probe (`plan.txt`, written and pushed before the run, `6dc27f5b` on `probe/h264-cabac-decision-pad`)

- **Change:** twelve independent one-uop instructions (`lea o, [value + value]`, output unused) at the start of
  every DecodeDecision (`probe-pad.diff`).
- **Check:** +11.94 instructions per regular bin, so nothing spilled (callgrind, 30 pictures of `a81de82106be`).
  Output is identical to the base.
- **Method:** `ab3.py` with 8 rounds over all 32 training streams at the class frame counts, one thread pinned to
  CPU 3, order rotated, CPU time.
- **Host:** a development VM, Intel Xeon @ 2.50 GHz, family 6 model 85. It is not the bar class, so the result is
  directional.
- **Results:** `ab-probe.txt` (named `ab.txt` in the plan), `ab-probe-summary.txt` (`abq.py`).

| Streams | probe over base | slower |
|---|---|---|
| CABAC (26) | **1.0229 [1.0174, 1.0292]** | 26 of 26 |
| CAVLC (6, no bins: the null control) | 1.0010 [0.9873, 1.0061] | 5 of 6 |
| all 32 | 1.0191 [1.0151, 1.0263] | 31 of 32 |
| sd, 720p, 1080p, 4k | 1.0076, 1.0162, 1.0434, 1.0239 | |

The rule (build the cut if CABAC ≥ 1.010 with its lower end above 1.000, and the null within ±0.005) is met.

## The cut (`v2.diff`, `v6.diff`; the plan's addendum, `e7e39c35`, was pushed before its timing)

- **Design:** one 8-byte `Step` per (state, codIRange quarter), indexed by `state << 2 | qCodIRangeIdx` from one base.
  It holds rangeLPS, its renormalized value and shift, and both next states. Its unit test checks every entry against
  Tables 9-44 and 9-45 and RenormD. Output is identical on the streams checked.
- **Instructions saved per regular bin** (callgrind, `a81de82106be`):

| Formulation | saved a regular bin |
|---|---|
| six branch-free forms | −1.6 to +2.3 |
| `v2` (`select_unpredictable`, above the workspace MSRV) | 1.55 |
| `v6` (`if`/`else`; LLVM branches on the bin) | 10.68 |

- **Timing:** `ab-c.txt`, `ab-c-summary.txt`; 6 rounds, same method and host.

| Arm | CABAC (26) | all 32 | CAVLC (6) |
|---|---|---|---|
| `v2` | 1.0010 [0.9926, 1.0066], 13 of 26 faster | 0.9995 [0.9928, 1.0036] | 0.9965 |
| `v6` | **1.0243 [1.0115, 1.0316]**, 4 of 26 faster | 1.0175 [1.0075, 1.0277] | 0.9960 |

Neither meets the request rule (all 32 at most 0.995 with the upper end below 1.000, and CABAC at most 0.995). Both
are on `rejected/` branches, as the plan says: `rejected/h264-cabac-decision-v2` and
`rejected/h264-cabac-decision-branch` (`v6`). `v2` showed no gain and needs a toolchain above the MSRV, so it is not
proposed for a bundle.

Binaries: `binaries.sha256`. Spend: none (local VM).
