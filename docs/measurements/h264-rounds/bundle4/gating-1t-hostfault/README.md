# Bundle 4 B4-4, one-thread arm: aborted on a host fault (2026-10-28)

base = main `1e7dd54b`, k = `5f7d9fee` (identity 32 of 32 at 1 and 8 threads, `identity.txt`); `run-1t-two-arms.sh` of `measured/h264-helper-setup` adapted to six rounds, `g` only, seed 20261030.
A VM of the 2.90 GHz class (Xeon, model 106). The host gate failed repeatedly: ten before-checks (FFmpeg 1.07 to 1.68 times the reference; the run stopped), one PASS, a call that started and was voided by its after-check (`g.sd.json.void1`), then the before-checks of attempt 2 failed again. No call produced a valid result; no B4-4 number exists from this VM. Stopped; B4-4 moved to another host (`../gating-1t-gcp/`). Spend: none.
