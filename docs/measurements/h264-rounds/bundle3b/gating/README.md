# Bundle 3b: the gating runs (B3b-1 to B3b-6)

Bars and method: `../plan.md`, "Bars", and `../gating-request.md`. Arms: `base` = `main` at `1e7dd54b`, `k` = the bundle (its code is `bd1f8b30`'s; the later commits before the runs changed comments and `docs/` only), null arm `base`. Each built with `cargo build --locked --profile dist --example decode -p coreav-h264`, `RUSTFLAGS` unset. FFmpeg 9.0.2 (BtbN `n9.0.2-17-g2a571b6068`) is the reference command.

Each bar is the median of the per-stream ratios `k` over `base` (each stream's time the median of its rounds) with a 95% bootstrap interval over streams. The bars are written as `bars-1t.json` and `bars-8t.json` (and, for the information runs, `bars-gcp-1t-train.json` and `bars-gcp-1t-heldout.json`); `python3 -I scripts/h264_parity_bars.py <bars> <directory>` reads every call's report and writes the matching `analysis-*.txt`.

## The runs that decide

| directory | bars | host and options | rounds | state |
|---|---|---|---|---|
| `1t/` (`g.*` training, `h.*` held-out) | B3b-1 to B3b-4 | the bars' class: a VM of the 2.90 GHz class (family 6, model 106, stepping 6), pinned to core 2; `scripts/h264_parity.py` with `--min-seconds 3 --shuffle-seed 20261029 --null-arm base --gate-stream 36501d448eaa.0.720p.h264 --gate-every 4`; the host-health gate before and after each call | 20 (training), 5 (held-out) | 16 of 16 host gates passed, no call voided; no stream unresolved (one held-out 4k stream timed again after a round spread of 1.51, as the harness does) |
| `gcp-8t/` | B3b-5, B3b-6 | Compute Engine `c4-standard-16`, one thread a core (Xeon Platinum 8581C); `scripts/h264_parity_gcp.py`, `--threads 8 --min-seconds 3 --seed 20261029` | 10 | not void: reference drift +4.4% (bar 5%), no stream unresolved, largest null deviation 1.8% |

The one-thread builds are bit-identical to the eight-thread run's (`1t/provenance.txt`, `gcp-8t/binaries-as-run.sha256`). Their output is identical at 1 and 8 threads on the 32 training and 64 held-out streams (`1t/identity.txt`, `gcp-8t/identity.txt`, 96 of 96 each).

## Result: every bar met

| bar | measure | result | bar value |
|---|---|---|---|
| B3b-1 | one thread, discard, all 32 | **0.9886** [0.9823, 0.9918], 30 of 32 faster | at most 0.990, upper end below 1.000: met |
| B3b-2 | one thread, discard, 64 held-out | **0.9934** [0.9899, 0.9958], 49 of 64 faster | at most 0.995, upper end below 1.000: met |
| B3b-3 | one thread, write, all 32 | **0.9891** [0.9847, 0.9929] | at most 1.000: met |
| B3b-4 | one thread, each class, discard and write | discard 0.9911, 0.9864, 0.9878, 0.9696; write 0.9928, 0.9863, 0.9916, 0.9715 (sd, 720p, 1080p, 4k) | at most 1.005: met |
| B3b-5 | eight threads, all 32 | discard **0.9984** [0.9943, 1.0003]; write **0.9994** [0.9961, 1.0020] | at most 1.005 each: met |
| B3b-6 | eight threads, each class | discard 1.0013, 0.9966, 0.9960, 0.9957; write 0.9985, 1.0011, 0.9977, 1.0011 | at most 1.010: met |

B3b-7 (output) is in `../checks.md`, and it was checked again on the tree merged with today's `main` (below).

Information: the time ratio to FFmpeg 9.0.2, a median per-stream ratio on each run's host, `base` then `k`. One thread, discard: training 0.973 then 0.972, held-out 0.956 then 0.951. One thread, write: training 0.943 then 0.930, held-out 0.946 then 0.943. Eight threads, discard: 0.849 then 0.843. Per class, and the count of streams below 1.000, are in `analysis-1t.txt` and `analysis-8t.txt`.

## A second one-thread run, on the Compute Engine class (information)

The one-thread result above arrived late. Before it did, the same arms were run once on the Compute Engine one-thread class (`docs/design/h264.md`, "The one-thread class of rounds whose bars are committed from here on"): `c4-standard-8` with one thread a core, `scripts/h264_parity_gcp.py --one-thread-class --pin 2 --seed 20261029`, 20 rounds on the training streams (`gcp-1t-train/`) and 5 on the held-out streams (`gcp-1t-heldout/`). These runs decide nothing, because the bars name the 2.90 GHz class. They agree with it: discard 0.9881 [0.9787, 0.9911] over the 32 training streams and 0.9886 [0.9859, 0.9915] over the 64 held-out streams, write 0.9878, every class at most 0.9925. A first held-out run there was void, because its three gate references disagreed by more than 5% (720p stream 1.227, 1.085, 1.229 s). It ran again on a new VM, and the void run's ratios were not read.

## The merged tree

`identity.txt`, `identity-summary.txt` and `identity-job.sh` (with `binaries.sha256`) check four sets of builds on the 96 streams at 1 and 8 threads. The first set is `base` and `k` as built for the Compute Engine runs. The others are the bundle merged with today's `main` against `main`, both with `fast` and without it. Output is identical in every case (96 of 96 each).
