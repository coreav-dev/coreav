# The residual's block dispatch: probe, cut and local timing

Design, bars and results: `docs/design/h264.md`, "Entropy round: the residual's block dispatch". Host: a development VM,
Intel Xeon @ 2.50 GHz (not the bar class; directional). Builds: `cargo build --locked --profile dist --example decode
-p coreav-h264`.

| name in the files | build |
|---|---|
| `base`, `bin-base` | `main` at `e2271b3a` |
| `pad`, `bin-pad` | `probe/h264-residual-calls-pad` at `fe7a043f` with `--features probe-pad` (`probe-pad.diff`) |
| `r3` | an intermediate form of the cut: one call a block, `nz` from the coded_block_flag bits, `quadrant` |
| `r4`, `k`, `bin-r4` | the cut as merged for gating, the code of `fded8560` (built before `cargo fmt` and the test `a_quadrant_holds_the_raster_bits_of_its_four_blocks` were added; neither changes the compiled decoder) |

- `plan.txt`: the probe's plan and rule, and the cut's local-timing rule, committed before the runs.
- `ab-probe.txt`, `ab-probe-summary.txt`: the probe's timing (`ab3.py`, eight rounds, one thread pinned, discard).
- `instructions.txt`: callgrind totals at a tenth of the class frame counts (`cg32.sh`), `base`, `pad`, `r3`, `r4`.
- `ab-k.txt`, `ab-k-summary.txt`: the cut's local timing (`ab3.py`, eight rounds); `abcls.py` reads both timings.
- `identity-r4.txt` (`ident.sh`): md5 of the output, 32 streams at 1 and 8 threads, `base` and `r4` (default features).
- `dispatch-lines-base.txt`, `dispatch-lines-k.txt` (`lines.py`): instructions a macroblock by source line in the
  macroblock layer's walk over the residual blocks, `1d4b1775d78a`, 20 pictures, line-table builds of `base` and `r4`.
