# H.264 decoder against FFmpeg 9.0.2: the official snapshot of 2026-10-29

CoreAV's H.264 decoder (the `decode` example of `coreav-h264`, built with `cargo build --locked --profile dist`, rustc 1.99.1) against FFmpeg 9.0.2 (the BtbN build `n9.0.2-17-g2a571b6068`, sha256 `0d709a0bed1a43f24cd2c420878c63afea4580671b6949ad19ed124572f3bc3d`), on the project's main as of 00:00 UTC on 2026-10-29. The measured H.264 code and harness are byte-identical to those of public main at 898bfa4 (unchanged since the initial release e669e5d7). Measured once; nothing was tuned after seeing a number. This page records numbers and gives no verdict on any bar.

## How to read the numbers

A ratio is CoreAV's time over FFmpeg's time for one stream, so below 1.00 means CoreAV is faster. Each cell is the median of the per-stream ratios, its 95% bootstrap interval over the streams in the cell (2,000 resamples), and the number of streams where CoreAV is faster than FFmpeg. A cell is a speed claim against FFmpeg only where its interval lies below 1.00; the per-class cells of eight streams are wide, and several straddle 1.00.

- **Streams.** The 32 training streams (8 per resolution class) and the 64 held-out streams (16 per class) of `docs/measurements/h264-training/` and `docs/measurements/h264-n2-heldout/`, as elementary streams of the first 1,800 (sd), 900 (720p), 300 (1080p) or 150 (4k) access units. The classes are sd, 720p, 1080p and 4k by luma samples.
- **Two pairings, both symmetric.** `discard`: CoreAV with `--discard` against `ffmpeg -f null -`. `write`: CoreAV writing its frames to a discarded stdout against `ffmpeg -f rawvideo -`. FFmpeg runs with `-fps_mode passthrough` and the same thread count as CoreAV.
- **Harness.** `scripts/h264_parity.py`: every command of a stream is timed in the same interleaved rounds after one warm-up, in a random order per round (seed 20261026), and a stream's time is the median of its rounds. The one-thread calls use 10 rounds on the training streams and 5 on the held-out streams, pinned to one core. The eight-thread calls are unpinned, use 10 and 5 rounds, and repeat each command until a timed call lasts at least 3 seconds (the time is per decode).
- **Null arm.** A second copy of CoreAV's binary is timed in the same rounds as the first; the median ratio of the copy over the original is the noise of the median. It was 0.998 to 1.001 in every cell.
- **Host checks.** Every call has an in-call gate: FFmpeg is timed on a reference stream before the first stream, after every fourth stream and after the last, and a stream timed while the gate had drifted by more than 5% is timed again. The one-thread calls are also bracketed by a host check (FFmpeg on two reference streams before and after each call; a call whose after-check fails is run again), and the eight-thread runs time the reference before the first call and after the last (the drift below). A stream whose own repeat timings disagree is timed again. The rules are in `docs/design/h264.md`, "Measurement method: host faults inside a call" and "Where the eight-thread arms run".

## Hosts

- **One thread:** a cloud VM of the Intel Xeon @ 2.90 GHz class (family 6, model 106, AVX-512), 8 vCPUs.
- **Eight threads:** a Compute Engine VM, `c4-standard-16` with one thread per core (8 vCPUs on 8 physical cores, no SMT), Intel Xeon Platinum 8581C @ 2.30 GHz, in `us-central1-a`.

The two hosts are different machines. One-thread and eight-thread numbers are each of their own host and are not compared with each other, and ratios to FFmpeg are of the machine they were measured on.

## One thread, pinned (2.90 GHz class)

| class | training discard | training write | held-out discard | held-out write |
|---|---|---|---|---|
| all | 0.974 [0.903, 1.002], 20/32 | 0.935 [0.891, 0.971], 26/32 | 0.958 [0.923, 0.985], 45/64 | 0.942 [0.909, 0.961], 53/64 |
| sd | 0.903 [0.787, 0.958], 7/8 | 0.897 [0.790, 0.932], 7/8 | 0.955 [0.918, 0.970], 14/16 | 0.951 [0.927, 0.969], 13/16 |
| 720p | 1.014 [0.971, 1.042], 2/8 | 0.985 [0.919, 1.031], 5/8 | 1.001 [0.980, 1.019], 8/16 | 0.978 [0.952, 1.004], 11/16 |
| 1080p | 1.012 [0.986, 1.047], 3/8 | 0.988 [0.968, 1.022], 6/8 | 0.974 [0.835, 1.033], 9/16 | 0.945 [0.808, 0.990], 13/16 |
| 4k | 0.863 [0.731, 0.977], 8/8 | 0.834 [0.671, 0.938], 8/8 | 0.871 [0.812, 0.923], 14/16 | 0.825 [0.771, 0.902], 16/16 |

## Eight threads (Compute Engine, one thread per core)

| class | training discard | training write | held-out discard | held-out write |
|---|---|---|---|---|
| all | 0.831 [0.750, 0.870], 32/32 | 0.843 [0.755, 0.883], 31/32 | 0.828 [0.764, 0.857], 62/64 | 0.832 [0.758, 0.857], 62/64 |
| sd | 0.872 [0.750, 0.914], 8/8 | 0.895 [0.757, 0.934], 7/8 | 0.874 [0.863, 0.908], 16/16 | 0.906 [0.861, 0.932], 16/16 |
| 720p | 0.843 [0.789, 0.919], 8/8 | 0.880 [0.792, 0.884], 8/8 | 0.861 [0.828, 0.914], 15/16 | 0.858 [0.830, 0.932], 15/16 |
| 1080p | 0.877 [0.747, 0.902], 8/8 | 0.866 [0.750, 0.891], 8/8 | 0.819 [0.621, 0.851], 15/16 | 0.812 [0.663, 0.851], 15/16 |
| 4k | 0.618 [0.466, 0.735], 8/8 | 0.633 [0.542, 0.726], 8/8 | 0.656 [0.591, 0.721], 16/16 | 0.664 [0.614, 0.706], 16/16 |

## Noise and faults

- **Null copy over CoreAV** (discard / write): one thread, training 0.998 / 1.001, held-out 1.000 / 0.999; eight threads, training 0.999 / 1.000, held-out 1.001 / 1.000.
- **No run is void and no stream ended unresolved.** One-thread: one call (the 4k training call) failed its after-check and was run again, and passed; 2 training and 10 held-out streams were timed again after a disagreement between their repeats. Eight threads: the reference stream's time moved by +4.5% between the first call and the last of the training run (+0.9% for the held-out run), both inside the 5% limit; one call (720p, training) had a confirmed drift of +5.0% in the in-call gate, and its 4 streams were timed again.
- The elementary streams match the recorded digests in the training and held-out stream lists.

## Evidence

The per-call result files are in `raw/`: per-stream timings of every round, the null-arm timings, each stream's spread and retries, and the in-call gate samples (`raw/one-thread/{g,h}.<class>.json`, `raw/eight-thread-training/t8.<class>.json`, `raw/eight-thread-heldout/t8.<class>.json`), the voided one-thread call kept as `g.4k.json.void1`, the host-check log of the one-thread run, the reference times and verdict of each eight-thread run, and the machine description (`host.txt`). `analyze-snapshot.py` recomputes every table: put the one-thread reports, and the two eight-thread directories' `t8.*.json` as `t8.*.json` (training) and `h8.*.json` (held-out), in one directory and run `python3 -I analyze-snapshot.py DIR`; its output is `raw/one-thread/analysis-1t.txt` and `raw/eight-thread-analysis.txt`. The files are as the runs wrote them, except that the VM's name is replaced by `<vm>` and home-directory paths are shortened to the file name.

## What this is not

It is a snapshot of one commit on two machines, not a promise for other machines: the ratio to FFmpeg moved by about 10% between the 2.90 GHz and the 2.50 GHz classes in an earlier calibration (`docs/measurements/h264-training/README.md`). The run logs, the job scripts and the cost records are not published.
