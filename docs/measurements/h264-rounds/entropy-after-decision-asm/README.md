# Entropy decoding and the MB layer against FFmpeg, after the residual dispatch and the assembly decision

The breakdown of `../cabac-decision-probe/` ("Entropy decoding: the instruction excess by syntax element"), repeated
on the code of `perf/h264-cabac-decision-asm` (`200f9a79`: change A's residual dispatch and change B's site `decision`).
Local, instruction counts only.

- **CoreAV:** `prof-200f9a79`, a `dist` build of `200f9a79` with `CARGO_PROFILE_DIST_DEBUG=line-tables-only`
  (`binaries.sha256`). Callgrind at instruction level on the 32 training streams at a tenth of the class frame counts
  (`run_core.sh`).
- **FFmpeg, bins and macroblock counts:** reused from `../cabac-decision-probe/` (`ff.pkl`, `bins.pkl`, `mbs.json`).
  FFmpeg did not change, bins depend on the stream, and the decoded macroblocks are the same (output identical).
- **Scripts:** those of `../cabac-decision-probe/breakdown/`, with paths moved (`*.py.diff`). `report.py`'s engine set
  adds `decision.rs:decision` and `cabac.rs:reserve`. The stage classifier (`classify.diff`) classes the v3
  instance's `residual` and `decision` wrappers and `fast/decision.rs` as entropy; without that the residual's code
  in the v3 instance fell out of the stages.
- **Results:**
  - `final.txt`: groups per macroblock against FFmpeg.
  - `shares.txt` (`shares.py`): each group's share and excess as a share of a stream's instructions, medians by
    entropy coder and class.
  - `cavlc-shares.txt` (`cavlc_shares.py`): per stream, the MB-level syntax read together (MB header, MB layer, mvd and
    ref_idx, CAVLC syntax), the residual, NAL/RBSP and the unassigned rest.
  - `detail-mvd.txt`, `detail-cavlc.txt` (`detail.py`): a category by innermost frame and line.

Summary, per macroblock, mean over streams:

| | CoreAV | FFmpeg | excess | was |
|---|---|---|---|---|
| CABAC (26): residual | 1,120 | 974 | +147 | +783 |
| CABAC: mvd and ref_idx | 267 | 126 | +141 | +186 |
| CABAC: MB header and loop, MB layer (read together) | 1,123 | 1,154 | −31 | +234 |
| CABAC: NAL/RBSP | 100 | 19 | +81 | +81 |
| CABAC: unassigned (wrappers, closures) | 130 | 0 | +130 | +93 |
| CABAC: total | 2,741 | 2,274 | **+467** | +1,377 |
| CAVLC (6): total | 1,491 | 834 | **+657** | +778 |

- **CABAC engine:** 22.5 instructions a bin, down from 36.8. FFmpeg's `get_cabac` is 27.6 a call.
- **Attribution:** the split between the residual and the MB-level syntax is not reliable stream by stream. On
  intra-heavy streams FFmpeg's `ff_h264_decode_mb_cabac` holds code CoreAV counts as residual (the MB-level groups
  read negative where the residual reads high). The totals are reliable.
- **CAVLC streams:** the entropy and MB-layer excess is 8% to 24% of the instructions of five of the six (−0.2% on
  `6f7799768413`). The largest single piece is the residual block parse (`cavlc.rs` `block_body`).
