# The slowest training streams against FFmpeg, by stream

Evidence for `docs/design/h264.md`, "Deblocking round: macroblocks below the thresholds" → "The slowest streams
against FFmpeg". Diagnosis only: the PMU host is not the bar class, so nothing here is a speed claim or moves a bar.

## Host and binaries

- Compute Engine `c4-standard-8`, SPOT, `us-central1-a`, PMU `STANDARD`, Debian 13 (kernel and perf 6.12.113):
  Intel Xeon Platinum 8581C @ 2.30 GHz, family 6 model 207 (`run/host.txt`). Created, run and deleted by
  `scripts/gcp_vm_job.py` (`run/gcp-job.log`), one VM, one run.
- The job is `../profile-acf3ee2f-pmu/job.sh ipc` with its `rec_pmu.py`, unchanged. Its attribution build of FFmpeg
  9.0.2 was made as in that directory: make's log was deleted unread, and the source tree, tarball and log were
  deleted before any profiling (`run/build-order.txt`). It has no `.debug_*` section (`run/ffattr-sections.txt`), so
  every FFmpeg number is at function level. Its output equals the BtbN build's (`run/ffattr-output.txt`).
- CoreAV: `prof-dd85b381`, `main` at `dd85b381` (its decoder is `acf3ee2f`'s), `cargo build --profile dist -p
  coreav-h264 --example decode` with `CARGO_PROFILE_DIST_DEBUG=line-tables-only`. The binary `an earlier change` profiled was
  not kept, and samples are file addresses, so its run could not be split by stream with another build. FFmpeg's
  timing reference is the BtbN build `n9.0.2-17-g2a571b6068` (`run/binaries.sha256`).
- The 32 training streams at the class frame counts, one thread pinned to CPU 3, two rounds.

## Files

- `per_stream.py RES CORE_BIN`: runs `../profile-acf3ee2f-pmu/stages_pmu.py` (its tables in `run/stages.txt`), then
  writes each stream's stages and functions (`RES/per_stream.json`) and prints `run/per-stream.txt`. Each line holds
  CoreAV's cycles and instructions over the BtbN build's (`perf stat`), then each stage's excess: CoreAV's sampled
  cycles less FFmpeg's, as a share of FFmpeg's.
- `causes.py RES/per_stream.json` → `run/causes.txt`. It ranks the streams by their bar-class ratio to FFmpeg
  (`../mb-layout-2/gating/analysis.txt`, call `g`, discard, `after`) and lists each one's coding features
  (`../snapshot-outliers/features.tsv`; bits a macroblock from `../../h264-training/es.json`). It gives the excess by
  group, the means over the ten slowest and over all 32, and the correlations with the features. Inter prediction,
  reconstruction and deblocking are split into a driver and kernels. CoreAV's kernels are its SIMD modules
  (`avx512.rs`, `x86.rs`, `chroma_mb.rs`); `predict.rs` and `prefetch.rs`, safe Rust in the kernels crate, count as
  driver. FFmpeg's drivers are its C functions around the assembly (`loop_filter`, `ff_h264_filter_mb_fast`,
  `ff_h264_loop_filter_strength_mmxext`, `mc_part_*`, `ff_prefetch_mmxext`, `hl_decode_mb_*`), and its kernels the
  rest. Within a stage only the sum compares like with like.
- `qp.sh FFMPEG ES_DIR` → `run/qp.txt`: each stream's slice QPs and mean FilterOffsetA over the class frame count,
  read from FFmpeg's `trace_headers` bitstream filter (a black box).

## Spend

0.12 USD (upper bound: 0.193 hours the VM existed times 0.60 USD an hour).

## Reproduce

```
# upload directory: ../profile-acf3ee2f-pmu/job.sh and rec_pmu.py, es/ (32 streams), prof-dd85b381, ffmpeg (BtbN)
python3 -I scripts/gcp_vm_job.py --upload UP --job "job.sh ipc" --download res --max-hours 1.5 --cap-usd 1
python3 -I per_stream.py res prof-dd85b381 > run/per-stream.txt 2> run/stages.txt
python3 -I causes.py res/per_stream.json > run/causes.txt
./qp.sh ffmpeg es > run/qp.txt
```

Raw data (`res/`, the attribution build's symbol table trimmed to function symbols `t`, `T`, `w`, `W`): `raw.tar.gz`
on branch `measured/h264-slow-streams` (`2ecd09d7`), sha256
`198bf5d0d61c82a3a54e85686d64e2bb1fae8558e37399a5a7151da6e3d6c15b`.
