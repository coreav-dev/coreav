# H.264 decoder against FFmpeg 9.0.2: the official snapshot of 2026-10-29

CoreAV's H.264 decoder (the `decode` example of `coreav-h264`, built with `cargo build --locked --profile dist`, rustc 1.99.1) against FFmpeg 9.0.2 (the BtbN build `n9.0.2-17-g2a571b6068`, sha256 `0d709a0bed1a43f24cd2c420878c63afea4580671b6949ad19ed124572f3bc3d`), on the project's main as of 00:00 UTC on 2026-10-29 (the project's internal commit `361cd29f`). Measured once; nothing was tuned after seeing a number.

## How to read the numbers

A ratio is CoreAV's time over FFmpeg's time for one stream, so below 1.00 means CoreAV is faster. Each cell is the median of the per-stream ratios, with a 95% bootstrap interval over the streams (2,000 resamples) where the cell covers every stream, and the number of streams where CoreAV is faster than FFmpeg.

- **Streams.** The 32 training streams (8 per resolution class) and the 64 held-out streams (16 per class) of `docs/measurements/h264-training/` and `docs/measurements/h264-n2-heldout/`, as elementary streams of the first 1,800 (sd), 900 (720p), 300 (1080p) or 150 (4k) access units. The classes are sd, 720p, 1080p and 4k by luma samples.
- **Two pairings, both symmetric.** `discard`: CoreAV with `--discard` against `ffmpeg -f null -`. `write`: CoreAV writing its frames to a discarded stdout against `ffmpeg -f rawvideo -`. FFmpeg runs with `-fps_mode passthrough` and the same thread count as CoreAV.
- **Harness.** `scripts/h264_parity.py`: every command of a stream is timed in the same interleaved rounds after one warm-up, in a random order per round (seed 20261026), and a stream's time is the median of its rounds. The one-thread calls use 10 rounds on the training streams and 5 on the held-out streams, pinned to one core. The eight-thread calls are unpinned, use 10 and 5 rounds, and repeat each command until a timed call lasts at least 3 seconds (the time is per decode).
- **Null arm.** A second copy of CoreAV's binary is timed in the same rounds as the first; the median ratio of the copy over the original is the noise of the median. It was 0.998 to 1.001 in every cell.
- **Host checks.** A gate that times FFmpeg on a reference stream runs around each one-thread call and inside each eight-thread call; a call whose check fails is run again, and a stream whose repeat timings disagree is timed again (`docs/design/h264.md`, "Measurement method").

## Hosts

- **One thread:** a cloud VM of the Intel Xeon @ 2.90 GHz class (family 6, model 106, AVX-512), 8 vCPUs.
- **Eight threads:** a Compute Engine VM, `c4-standard-16` with one thread per core (8 vCPUs on 8 physical cores, no SMT), Intel Xeon Platinum 8581C @ 2.30 GHz, in `us-central1-a`.

The two hosts are different machines. One-thread and eight-thread numbers are each of their own host and are not compared with each other, and ratios to FFmpeg are of the machine they were measured on.

## One thread, pinned (2.90 GHz class)

| class | training discard | training write | held-out discard | held-out write |
|---|---|---|---|---|
| all | 0.974 [0.903, 1.002], faster on 20/32 | 0.935 [0.891, 0.971], 26/32 | 0.958 [0.923, 0.985], 45/64 | 0.942 [0.909, 0.961], 53/64 |
| sd | 0.903, 7/8 | 0.897, 7/8 | 0.955, 14/16 | 0.951, 13/16 |
| 720p | 1.014, 2/8 | 0.985, 5/8 | 1.001, 8/16 | 0.978, 11/16 |
| 1080p | 1.012, 3/8 | 0.988, 6/8 | 0.974, 9/16 | 0.945, 13/16 |
| 4k | 0.863, 8/8 | 0.834, 8/8 | 0.871, 14/16 | 0.825, 16/16 |

## Eight threads (Compute Engine, one thread per core)

| class | training discard | training write | held-out discard | held-out write |
|---|---|---|---|---|
| all | 0.831 [0.750, 0.870], faster on 32/32 | 0.843 [0.755, 0.883], 31/32 | 0.828 [0.764, 0.857], 62/64 | 0.832 [0.758, 0.857], 62/64 |
| sd | 0.872, 8/8 | 0.895, 7/8 | 0.874, 16/16 | 0.906, 16/16 |
| 720p | 0.843, 8/8 | 0.880, 8/8 | 0.861, 15/16 | 0.858, 15/16 |
| 1080p | 0.877, 8/8 | 0.866, 8/8 | 0.819, 15/16 | 0.812, 15/16 |
| 4k | 0.618, 8/8 | 0.633, 8/8 | 0.656, 16/16 | 0.664, 16/16 |

## Noise and faults

- **Null copy over CoreAV** (discard / write): one thread, training 0.998 / 1.001, held-out 1.000 / 0.999; eight threads, training 0.999 / 1.000, held-out 1.001 / 1.000.
- **No run is void and no stream ended unresolved.** One-thread: one call (the 4k training call) failed its after-check and was run again, and passed; 2 training and 10 held-out streams were timed again after a disagreement between their repeats. Eight threads: the reference stream's time moved by +4.5% between the first call and the last of the training run (+0.9% for the held-out run), both inside the 5% limit; one call (720p, training) had a confirmed drift of +5.0% in the in-call gate, and its 4 streams were timed again.
- The elementary streams match the recorded digests in the training and held-out stream lists.

## What this is not

It is a snapshot of one commit on two machines, not a promise for other machines: the ratio to FFmpeg moved by about 10% between the 2.90 GHz and the 2.50 GHz classes in an earlier calibration (`docs/measurements/h264-training/README.md`). The per-call raw reports stay with the project's measurement archive; the numbers above are the medians they give.
