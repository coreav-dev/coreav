# VP9 through libvpx

`coreav thumbnail` and `coreav transcode` built with `libvpx` decode VP9 with libvpx in the sandboxed worker (`docs/design/decode.md`, "libvpx"). This page holds the results of the bars and method committed before these runs (`docs/design/decode.md`, "Bars", item 5, "For VP9 with libvpx"). The baseline is FFmpeg's own VP9 decoder (`vp9 (native)`, recorded in every job's `host.txt`), never its libvpx wrapper.

## Verdicts

| Bar | Result | Verdict |
|---|---|---|
| Bar 1, frames (99% of streams, 99.9% of frames) | 74 of 74 streams, 8,408 of 8,408 frames equal to `framemd5` with `-c:v vp9` | met |
| S1, keyframe mode (median at most 1.00) | 0.254 (0.226 to 0.290), 100% of files below 1.00 | met |
| S2, keyframe mode, same frame | 0.549 (0.513 to 0.622), 99% | met |
| S3, `--exact` | 0.994 (0.895 to 1.053), 51% | met |
| S4, PNG | 0.646 (0.590 to 0.678), 99% | met |
| S5, WebP | 0.241 (0.211 to 0.262), 100% | met |
| B5, transcode, `ffmpeg` arm (median at most 1.05) | 1.075 (1.038 to 1.123) | **missed** |
| B5, transcode, `x264` arm | 1.074 (1.040 to 1.109) | **missed** |

Medians of per-file ratios, CoreAV over FFmpeg, with bootstrap 95% intervals (10,000 resamples). The thumbnail files are both partitions of corpus v1's VP9 files (74: 46 development, 28 held-out). Earlier rounds timed the held-out ones through the fallback, so this is a development measurement: its verdicts stand, and it backs no claim that CoreAV is faster than FFmpeg. S3's interval reaches above 1.00 and run-to-run noise is about ±5%, so S3 is met but close.

## Hosts and builds

- Compute Engine `c4-standard-8` with simultaneous multithreading off (`smt active: 0`, 4 cores, one thread each), Intel Xeon Platinum 8581C, Debian 13, kernel 6.12.113. Each job alone on its VM, standard (not preemptible) provisioning.
- FFmpeg 7.0.2, the pinned static build (`scripts/install-oracle-ffmpeg.sh`). Its default VP9 decoder is `vp9 (native)`; it also has `libvpx-vp9`.
- libvpx 1.15.0 from Debian 13 (`libvpx9`) on the VMs. The builds were compiled against libvpx 1.14's headers (Ubuntu 24.04); both declare decoder ABI version 12, which libvpx checks when the decoder opens. The summaries name `libvpx 1.15.0` for every file.
- Builds of `feat/vp9-libvpx` at `85ffeb7`: `coreav` with `libvpx,ffmpeg-fallback` (the `libvpx` arm) and with `ffmpeg-fallback` alone (the `fallback` arm), `decode-oracle` with `libvpx`, and for B5 `coreav` with `native-h264,dav1d,libvpx,ffmpeg-fallback`. Their SHA-256 are in each job's `host.txt`. After these runs, the backend stopped copying the images of packets marked `discard`, which `--exact` sends before its target; that change was not measured.

## Thumbnails (S1 to S5)

`scripts/vp9_speed.py` through `scripts/vp9_speed_gcp.py`, with the method of `docs/design/thumbnails.md`, "Bars": per file one warm-up of each command, then 5 rounds in rotated order, each command a process timed from outside, outputs on tmpfs. The gate is FFmpeg's S1 command on one VP9 file, 3 runs after a warm-up, before and after; void above 5%.

**How the run was split.** The run was planned as one job. Its first attempt produced no results: the controller that holds the job's connection stopped before the job finished, and the VM deleted itself at its time limit. The files were then split into seven jobs of under an hour each, each on its own VM of the class above with its own gate before and after (`vp9_speed_gcp.py --sha`; `vp9_speed.py merge` joins them). Each file's ratios come from one job, so they compare commands run on the same VM. Three jobs were void by their gate (−8.6%, −5.7%, −17.3%; `thumbnails/void/`) and were run again once each, as the rules allow for a failed host gate. The gates of the seven jobs counted moved −1.2%, −3.1%, −1.2%, +1.6%, +1.1%, +1.0% and −2.7%. The largest file (3.0 GB) went alone in a job without the decode-only timing.

| Bar | `libvpx` arm, all (74) | development (46) | held-out (28) | `fallback` arm, all | libvpx / fallback |
|---|---|---|---|---|---|
| S1 | 0.254 (0.226 to 0.290) | 0.254 | 0.226 | 0.294 | 0.846 |
| S2 | 0.549 (0.513 to 0.622) | 0.596 | 0.499 | 0.631 | 0.846 |
| S3 | 0.994 (0.895 to 1.053) | 1.030 | 0.915 | 1.114 | 0.899 |
| S4 | 0.646 (0.590 to 0.678) | 0.651 | 0.597 | 0.719 | 0.868 |
| S5 | 0.241 (0.211 to 0.262) | 0.243 | 0.228 | 0.287 | 0.829 |

One development file is left out of S1 and S3: FFmpeg's command at its target time wrote no frame. Every file was decoded by the backend its arm names, both arms chose the same frame on every file, and no S2 or S3 frame is under 30 dB against FFmpeg's.

S3 split by FFmpeg's own time for the command:

| FFmpeg's time | Files | `libvpx` arm | Below 1.00 | `fallback` arm | libvpx / fallback |
|---|---:|---:|---:|---:|---:|
| under 50 ms | 20 | 1.041 | 45% | 1.075 | 0.963 |
| 50 to 300 ms | 35 | 1.013 | 49% | 1.129 | 0.875 |
| over 300 ms | 18 | 0.894 | 61% | 1.118 | 0.793 |

Through libvpx, S3 is about 10% faster than through the fallback, and the gain grows with the decoding work.

**Decoder alone** (information, no bar): per file, `ffmpeg -threads 0 -c:v libvpx-vp9 -i F -map 0:v:0 -f null -` over the same with `-c:v vp9`, a warm-up and 3 rounds. Median 1.036 (0.979 to 1.106) over 72 files: 1.088 on the development files, 0.958 on the held-out ones. FFmpeg's `vp9` decoder failed on one file. The third command, `decode-oracle --packets N`, hashes every frame (the MD5 of its planes) and receives every frame from the worker, so it times the oracle and not the decoder: on a 1080p test stream, FFmpeg's own `-f framemd5` takes 2.4 times as long as its `-f null`. Its ratios are in `summary.md` and are not read here.

Evidence: `vp9-libvpx/thumbnails/`: `files.jsonl` and `oracle.jsonl` (each row names its job), `gate.json`, `summary.json` and `summary.md`, `selection.json`, each job's `host.txt`, `job.sh` and logs under `jobs/`, and the void jobs under `void/`.

## Frames (bar 1)

`vp9_speed.py oracle`, in the same jobs, after the timing: the first 120 packets of each file's video stream through `decode-oracle --backend libvpx`, the MD5 of each frame against FFmpeg's `framemd5` of the same stream decoded with `-c:v vp9`. 74 of 74 streams and 8,408 of 8,408 frames agree (46 development, 28 held-out). The streams cover profiles 0 to 3: 64 are 4:2:0 at 8 bits, 7 are 4:4:4 at 8 bits, 2 are 4:2:0 at 10 bits and 1 is 4:2:2 at 10 bits.

## Transcode (B5)

`scripts/transcode_speed_gcp.py --classes vp9 --threads-per-core 1` on the speed selection's 10 VP9 files (`docs/measurements/transcode/speed-selection.json`), with the method of `docs/design/transcode.md`, "Bars", B5 (`--preset medium --crf 23`), in one job on the host class above. The gate (the baseline without audio on a 20 s 720p `testsrc2` file encoded with libx264) took 2.767 s before and 2.825 s after, 2.1% apart.

| Arm | Median (95% CI) | Files faster than FFmpeg | Through the fallback, round 2 |
|---|---|---|---|
| `ffmpeg` | **1.075** (1.038 to 1.123) | 0 of 10 | 1.099 |
| `x264` | **1.074** (1.040 to 1.109) | 0 of 10 | 1.095 |

libvpx decoded every file. B5 is missed on both arms. libvpx moves VP9 about 2.5% closer to FFmpeg than the fallback did; the encoder takes most of each run.

Evidence: `vp9-libvpx/transcode-b5/`.

## The read on a native VP9 decoder

The rule was fixed with the bars: when the libvpx build misses S3 or B5 and the decoder-alone median is at or below 1.10, the cost is around the decoder, and a native decoder is not the first change to make. The libvpx build meets S1 to S5 and misses B5, and the decoder-alone median is 1.036. So the thumbnail bars need no native VP9 decoder, and for B5 a native decoder is not the first change: libvpx alone is within 4% of FFmpeg's own decoder, so most of the 7.5% that CoreAV's transcode takes over FFmpeg's is spent outside the decoder.
