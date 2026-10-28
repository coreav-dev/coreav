# Gating run of the bS kernel round, the one-thread VM

`base` = `bb7255cd`, `k` = `193051ba`, bars BS-1 to BS-5 of `docs/design/h264.md`, "Kernel round: boundary strengths in vector registers".
One thread pinned to core 2, twenty rounds, per class, `run-ab-helper.sh bb7255cd 193051ba` of the branch (unchanged), on a VM of the 2.90 GHz class (family 6, model 106, 8 CPUs;
`provenance.txt`; its null-arm calibration is `docs/measurements/h264-rounds/helper3-vm/calibration/` of branch `measured/h264-helper3-calibration`). Host gate before and after every call (`hostgate.log`).

- Identity (BS-5): 32 of 32 streams byte-identical to `base` with `k` at 1 and 8 threads (`identity.txt`). Conformance 97 of 97 is the branch's `conformance.txt`, not repeated here.
- **Void call**: the first `g.4k` call (`g.4k.json.void1`, `g.4k.log.void1`) ended with 8 unresolved streams, above the bars' four ("More than four unresolved streams in its calls make the run a host fault"): its in-call gate's
  baseline read 0.184 s and every later sample 0.120 to 0.157 s (-14.6% to -34.8%), though the host gates before and after the call passed. The call ran while the helper started the Compute Engine run of another round (uploads) and built it on cores 4 to 7
  (nice 19) from about 20 minutes into it; both are recorded as possible causes. It was run again alone, same binaries and options, nothing else running (`rerun-4k.sh`, attempt 2, 0 unresolved, in-call gate within 1.5%). The other three calls are the first and only ones of their classes.
  The first call's k/base per-class medians were close (4k discard 0.9783, write 0.9861), so the re-run does not change the verdict.
- `analysis.txt`: `analyze.py` over these reports and the Compute Engine ones (`../gating-gcp/`).

Results (median of the per-stream ratio `k / base`, 95% interval over streams):

| | 1 thread | 8 threads (`c4-standard-16`) |
|---|---|---|
| discard, 32 streams | **0.9802** [0.9749, 0.9832], 31/32 faster | **0.9891** [0.9862, 0.9913], 32/32 |
| write | 0.9829 [0.9769, 0.9839], 32/32 | 0.9903 [0.9890, 0.9939], 30/32 |
| discard by class: sd / 720p / 1080p / 4k | 0.9714 / 0.9832 / 0.9901 / 0.9792 | 0.9846 / 0.9900 / 0.9921 / 0.9906 |

BS-1 (at most 1.000 and the upper end below 1.000: 0.9832) pass; BS-2 (write 0.9829) pass; BS-3 (every class at most 1.005) pass; BS-4 (8 threads at most 1.002: discard 0.9891, write 0.9903) pass; BS-5 identity 32/32 (conformance as the branch's). Against FFmpeg 9.0.2 (1 thread, pinned, median of the per-stream ratio, information): `base` discard 1.094, `k` 1.072 (14 of 32 faster than FFmpeg); write 1.048 and 1.024.
The null arm's largest deviation of a stream: 0.4% sd, 1.2% 720p, 2.5% 1080p, 0.7% 4k (the bar is 4%).
