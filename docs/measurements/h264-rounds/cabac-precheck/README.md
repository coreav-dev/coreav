# CABAC pre-check (information only, no bars)

Four builds in the same rounds on the Intel 2.90 GHz development VM (family 6 model 106, `--require-cpu`, gate passes 9, one failed after-check voided the 4k call once, re-run, REBASE 0): `base` = main `27b504ab`;
`bmi2all` = main `27b504ab` built with `RUSTFLAGS='-C target-feature=+bmi2'`; `e1` = `exp/h264-cabac-levels` `6e8f7a70`; `b1` = `exp/h264-cabac-bmi2` `23f6877d`. Eight CABAC streams (two per class), one thread, `--pin 2 --rounds 5`
(FFmpeg 9.0.2 was in the call and is not used). Identity: all builds' output equals base on the 32 training streams at one thread (`identity.txt`). Ratios to base per stream: `precheck-ratios.json`. Build outputs: `binaries.sha256`. Spend: none.

Median after/base, discard (write): bmi2all 0.9769 (0.9803), e1 0.9978 (1.0003), b1 0.9975 (0.9972).

`analyze.py` (`python3 -I analyze.py .`, output in `analysis.txt`) recomputes the medians above from the four `g.*.json`, with each class, `b1` against `e1`, and `base` against FFmpeg 9.0.2.
The pre-check's base is `main` at `27b504a`; the local split below uses `main` at `c17ae06`, five merged commits later (none in the CABAC code).

`local/`: on a development VM with the same CPU model (Intel Xeon @ 2.90 GHz, family 6, model 106, stepping 6; a different VM, not a bar run), `perf` cpu-clock samples per function
(`symdiff.sh`, summed by `symsum.py`; raw samples `sym-*.raw`, summary `functions.txt`): `main` at `c17ae06` with and without `-C target-feature=+bmi2`, and `cabac-e1` against `cabac-b1`
(local builds of `6e8f7a70` and `23f6877d`). `b1-shifts.txt` counts, per residual function of the local `b1` build, the BMI2 shifts and the shifts by CL.
