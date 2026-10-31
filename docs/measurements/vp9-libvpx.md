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
- Builds of `feat/vp9-libvpx` at `85ffeb7`: `coreav` with `libvpx,ffmpeg-fallback` (the `libvpx` arm) and with `ffmpeg-fallback` alone (the `fallback` arm), `decode-oracle` with `libvpx`, and for B5 `coreav` with `native-h264,dav1d,libvpx,ffmpeg-fallback`. Their SHA-256 are in each job's `host.txt`. After these runs, the backend stopped copying the images of packets marked `discard`, which `--exact` sends before its target, and began giving libvpx every packet with a superframe index, which copies a packet without one once (its compressed bytes, not a frame). Neither change was measured.

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

## Attribution of the B5 gap

Where the extra time of the transcode goes, measured after B5 on main at `7304293` against FFmpeg 9.0.2, the pinned build (`scripts/oracle-ffmpeg.env`, checked with `oracle_pin.py --verify-bin`). This is a diagnosis on the 10 files B5 timed, not a run of the bar: it judges nothing. The method and the arms were written down before the run.

**Host and builds.** One `c4-standard-8` as above (Intel Xeon Platinum 8581C, SMT off, 4 cores, standard provisioning), Debian 13 with libvpx 1.15.0 (`libvpx9`) and `perf` 6.12. `coreav` built with `--release` and `native-h264,dav1d,libvpx,ffmpeg-fallback`; the x264 program from `scripts/install-x264.sh`. For the decode-alone arms, `tools/decode-oracle` was built with `libvpx` and, in a local copy that is not committed, without its per-frame MD5 and packed copy, so it decodes to nothing.

**Arms.** Each command a process tree timed from outside, with the user and system CPU time of the tree; per file, the source in the page cache, outputs on tmpfs, 3 rounds in rotated order, a command's time the median of its rounds.

| Arm | Command |
|---|---|
| `base`, `base2` | B5's baseline, twice (A/A) |
| `c_ff`, `c_x264` | `coreav transcode` with `--encoder ffmpeg` and `--encoder x264` |
| `f_split` | FFmpeg's decoder in one process, raw frames as NUT through a pipe, FFmpeg's libx264 in a second process |
| `f_dec_vp9`, `f_dec_vpx` | `ffmpeg -f null` with its own `vp9` decoder, and with its own `libvpx-vp9` |
| `c_dec_ip`, `c_dec_w` | CoreAV's libvpx backend in the process, and in the sandboxed worker with frames through shared memory |
| `f_enc`, `x_enc` | libx264 in FFmpeg, and the x264 program with `coreav transcode`'s arguments, on the decoded frames from a tmpfs file (the 7 files whose raw video fits 4 GB); `*_pipe`: the same through a pipe |
| `*_an` | the two files with audio, without it (`-an`, `--audio none`) |

Then one profiled pass: `perf record -a -e cpu-clock` of the full commands and the decode arms, each sample counted by process, thread, library and function (none for FFmpeg's own code). FFmpeg names its threads (demuxer, decoder, filter, encoder, muxer), x264's threads are those its encoder thread starts, and CoreAV's are the worker, its threads, the main thread, the encoder pipe threads and the encoder process. On 4 cores, 4 × wall = CPU + idle, so the gap splits exactly into each stage's extra CPU over 4 and the change in idle time; a stage's CPU is the command's measured CPU times the stage's share of its samples. The profile's CPU and the timed CPU agree within 3% on every full command.

**Host checks.** The baseline's gate file took 2.864 s before and 2.852 s after (0.4% apart). `base2` over `base`: 0.996 (0.982 to 1.004).

### Results

| Ratio (median of 10 files) | Median | 95% CI | Files below 1.00 |
|---|---|---|---|
| `c_ff` / `base` | 1.074 | 1.017 to 1.121 | 2 |
| `c_x264` / `base` | 1.074 | 1.019 to 1.109 | 2 |
| `f_split` / `base`, audio matched | 1.025 | 1.011 to 1.070 | 1 |
| `c_x264` / `f_split`, audio matched | 1.035 | 0.994 to 1.056 | 3 |
| `x_enc` / `f_enc` (7 files) | 0.966 | 0.917 to 0.975 | 6 |
| `c_x264_an` / `c_x264` (2 files) | 1.002 | | 0 |

The two files that are faster are the two 400x250 files of about 0.11 s, where process start-up is most of the time. `f_split` carries no audio, so on the 2 files with audio both rows compare it with the commands without audio (`base_an`, `c_x264_an`). Splitting FFmpeg's own command into two processes over a pipe costs 2.5%; CoreAV adds another 3.5% on top of that.

Per stage, the extra time of the CoreAV commands over the baseline. Seconds are summed over the 10 files (their share of the summed gap in brackets); the percentages are the median over files of each stage's extra time as a share of the baseline's time, so they do not add up to the median gap.

| Stage | `c_x264`, seconds | `c_x264`, median file | `c_ff`, seconds | `c_ff`, median file |
|---|---|---|---|---|
| Decode | +7.05 (23%) | +0.2% | +7.27 (24%) | +0.2% |
| Frames to the encoder | +21.94 (71%) | +3.9% | +23.08 (75%) | +4.4% |
| Encode | +2.26 (7%) | −0.1% | +2.93 (9%) | +0.8% |
| Mux and demux | −0.11 | 0.0% | −0.08 | 0.0% |
| Start-up and other | +1.05 (3%) | +0.7% | +0.48 (2%) | +0.3% |
| Idle cores | −1.08 (−3%) | −0.8% | −2.78 (−9%) | −1.9% |
| Gap | +31.11 | +7.4% | +30.90 | +7.4% |

Per file, `c_x264`, each stage's extra time as a share of the baseline's (`c_ff`'s gap beside it):

| File | Size | Baseline (s) | Gap | `c_ff` gap | Decode | Frames | Encode | Other | Idle |
|---|---|---|---|---|---|---|---|---|---|
| `f51a56ed5718` | 3840x2160 4:2:0 | 99.79 | +6.9 | +6.4 | +2.5 | +3.7 | +1.4 | +0.1 | −0.7 |
| `a084bd45168c` | 400x250 4:4:4 | 0.11 | −1.7 | −1.3 | −0.6 | +3.0 | −3.4 | +0.7 | −1.4 |
| `70eec00bc7d2` | 640x352 4:2:0 | 2.85 | +3.1 | +1.7 | +1.0 | +2.2 | +0.5 | +0.5 | −0.9 |
| `9ffa165e752a` | 8000x4500 4:2:0 | 107.99 | +15.1 | +14.8 | +2.6 | +11.3 | +0.8 | +0.6 | −0.2 |
| `584a16b906bb` | 1217x689 4:4:4 | 0.99 | +13.1 | +16.6 | +0.3 | +11.8 | −0.1 | +3.6 | −2.4 |
| `ea684713a5be` | 3840x2160 4:2:0 | 82.81 | +8.7 | +9.4 | +2.1 | +6.5 | −0.1 | +0.2 | +0.1 |
| `bb5d39d47357` | 2548x1354 4:2:0 | 1.85 | +8.7 | +11.9 | −1.4 | +11.9 | −0.5 | +1.8 | −3.0 |
| `b267a2ddac45` | 1920x1080 4:2:0 | 6.47 | +5.4 | +5.9 | +0.1 | +3.9 | +1.5 | +0.5 | −0.6 |
| `f944c05afaa1` | 400x250 4:4:4 | 0.11 | −2.2 | −2.2 | −0.7 | +3.2 | −3.1 | +0.8 | −2.3 |
| `59ad8ee403b3` | 640x480 4:2:0 | 0.18 | +7.9 | +8.5 | +0.2 | +3.9 | −3.3 | +2.9 | +4.4 |

On the files under 0.2 s the split is noisy: their profiles are 20 runs each, and process start-up is spread over every stage.

### What the gap is

- **It is extra work, not stalls.** Idle core time goes down in CoreAV's runs (−0.8% and −1.9% of the baseline, median): its stages overlap at least as well as FFmpeg's, and every core stays busy, so each extra CPU second costs a quarter of a second.
- **Frames to the encoder: about 70% of the extra seconds.** Each frame is copied five times on its way to x264: from libvpx's image into a buffer in the worker, from that buffer into a shared-memory slot, from the slot into a buffer in the main process, into the pipe, and out of the pipe in the encoder. FFmpeg's single command has none of these steps between its decoder and libx264. In the median file the copies through the worker cost 2.4% of the baseline's time (4.5% on the 8000x4500 file) and the pipe write 0.7%. The rest is fresh pages zeroed by the kernel: 4.7% on the 8000x4500 file, whose 54 MB frames are above glibc's largest `mmap` threshold (32 MiB), so each frame's buffers are mapped and zeroed again (an inference from the profile: the zeroing is there at 8K and not at 4K), and, on the files under a second, likely the start-up of three processes instead of one. With `--encoder ffmpeg` the stage is a little larger (median +4.4% against +3.9%): FFmpeg reads the raw Matroska stream where the x264 program reads raw planes.
- **Decode: about 23% of the extra seconds, all on the large files.** In the transcode, Debian's libvpx 1.15.0 takes 17 to 27% more CPU than FFmpeg's own `vp9` decoder on the 4K and 8K files (2.1 to 2.6% of the baseline's time); on the smaller files it costs between 13% less and 14% more, at most 1% of the baseline's time. Decoding alone, by the CPU of the profiled pass, `c_dec_ip` takes 1.28 times `f_dec_vp9`'s CPU (median), and FFmpeg's own build of libvpx 1.07 times; the worker adds another 1.18 times on top of the in-process CPU. The timed CPU is not used for these arms: the harness does not always wait for its worker, so `c_dec_w`'s CPU in `timed.jsonl` sometimes leaves the worker out (the profile counts it; `stages.json`, `decode_alone_profile_cpu`). On wall time alone libvpx looks faster (`c_dec_ip` / `f_dec_vp9` 0.908), because it spreads one frame over every core, which a transcode does not leave free.
- **Encode: within about 1%.** x264 runs with the same options on both sides: its settings string in every output has `threads=6 lookahead_threads=1 sliced_threads=0 rc_lookahead=40` and the same values for every other option, except `keyint_min`, which follows the frame rate x264 is told. With `--encoder x264` it is 25 on every file where the baseline has 5, 5, 20, 23 or 10 (6 files), and with `--encoder ffmpeg` on 3 of those files. On the raw frames alone the x264 program is 3.4% faster than FFmpeg's libx264, and inside the transcode the encode stage is −0.1 to +1.4% of the baseline's time on the 4K and 8K files; where it is higher, sharing memory bandwidth with the copies is a likely cause, not measured.
- **Mux and audio: none.** Without the audio, both commands change by less than 1.1%, and the Annex B reader, spool file and MP4 writer are under 0.1%.

### A native VP9 decoder

The decode stage is 5 to 18% of a CoreAV transcode's CPU (10 to 17% on the 4K and 8K files). A native decoder with the CPU cost of FFmpeg's `vp9`, in the process, would remove the extra decode CPU and the copies through the worker. Turning that CPU into wall time assumes the idle time stays as it is, so these are estimates, not measurements: the median gap would go from 7.4% to about 2.3% with `--encoder x264` and 3.1% with `--encoder ffmpeg` (the pipe, the remaining page faults and start-up stay). Of those 5 points, only the decode part, 0.2% in the median file and 2 to 2.6% on the 4K and 8K files, needs a new decoder; the copies through the worker can be removed with libvpx where it is.

Evidence: `vp9-libvpx/attribution/`: `timed.jsonl` (every run: arm, round, wall, user and system time, and the commands), `stages.json` (per file and arm: median wall and CPU, CPU per stage, the gap per stage, the parts of the frames stage, and x264's options; the ratios above with their intervals), `gate.json`, `host.txt` and `instance.json`.
