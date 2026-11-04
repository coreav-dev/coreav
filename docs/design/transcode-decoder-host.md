# Transcode decoder-host diagnosis

This experiment asks how much of the VP9 and AV1 transcode overhead comes from hosting libvpx and dav1d in the sandboxed worker rather than in the calling process. It uses B5's method on previously measured inputs. It is a diagnosis, not a run of a bar, and judges nothing. It changes neither the default isolation policy nor the need for a native decoder.

The plan and its tool must merge before any command opens a registered input. Synthetic fixture checks are allowed before merge. Results follow in a separate measurement page. A failed command or incomplete roster produces an incomplete diagnosis, never a smaller successful cohort. A repeat is allowed only for a recorded host fault, including a failed host gate, not for an unfavorable result.

## Inputs

Use exactly the ten VP9 files and six AV1 files with `status: timed` in `docs/measurements/transcode/round-2/speed/files.jsonl`, in their recorded order within each class. Their full SHA-256 identities, extensions and stream maps are frozen in `bench/transcode-decoder-host/inputs.json`. These are the timed members of `docs/measurements/transcode/speed-selection.json`, including its top-up, not a new selection. Verify every file's complete SHA-256 before any media command. No replacement, reserve, new draw or exclusion is allowed.

## Host and builds

Use an on-demand, standard-provisioned GCP `c4-standard-8`, SMT off, four physical cores, matching the attribution run in `docs/measurements/vp9-libvpx.md`. Record the CPU model, kernel, operating system, effective affinity, SMT status, memory, library versions, build flags, source commit and executable SHA-256s. Refuse a different host class, active SMT or fewer than four available cores. Do not pin commands to fewer cores. Use Debian 13 and its libvpx 1.15.0 and dav1d 1.5.1, with the same libraries in both CoreAV arms.

Build merged CoreAV with `--release --features native-h264,dav1d,libvpx,ffmpeg-fallback`, without `native-vp9`, and no profile or compiler overrides. Both hosting arms use the same executable and decoder thread defaults. Retain a build receipt binding the clean merged source commit, build command and executable digest. The runner refuses a source commit not reachable from `origin/main` and a dirty tracked tree. Use FFmpeg and ffprobe from `scripts/oracle-ffmpeg.env`, checked by `oracle_pin.py --verify-bin`, and x264 built by `scripts/install-x264.sh` from that pin. Retain their hashes and version output. Build and download work stays outside the timed commands.

## Arms and commands

For each file run six commands, with `--preset medium --crf 23`, no scaling and the recorded video and copied-audio stream maps.

| Arm | Command |
|---|---|
| `base`, `base2` | B5's FFmpeg baseline, twice, as the A/A control |
| `sandbox_ffmpeg`, `sandbox_x264` | `coreav transcode IN OUT --encoder ffmpeg` or `--encoder x264`, with the diagnostic environment variable absent |
| `process_ffmpeg`, `process_x264` | The same commands with `COREAV_DIAG_TRANSCODE_IN_PROCESS=1` |

The baseline is `ffmpeg -nostdin -noautorotate -flags +unaligned -i IN -map 0:VIDEO`, followed by each recorded `-map 0:AUDIO`, then `-c:v libx264 -preset medium -crf 23 -c:a copy -fps_mode passthrough -y OUT.mp4`. CoreAV uses explicit absolute `--ffmpeg` and `--x264` paths and `--force`. The diagnostic variable is removed from every other command's environment. Strict-safe mode refuses the diagnostic switch rather than weakening its guarantees. The ordinary `coreav_decode::open` admission, packet and frame checks remain in force. In-process C code has no independent address-space or CPU limit and cannot be killed independently of the caller. This is an explicit diagnostic limitation, not a policy exception for normal runs.

Prime each source into the page cache twice. Use a dedicated tmpfs for every output. Time the complete command from outside, including startup, decoder, encoder and final muxing, with a finite timeout. Each file has three rounds in rotated order. The initial order is the table order, with the two sandbox commands before the two process commands. Round `r` rotates the six-element order by `r`; advance the starting offset by one for each subsequent file. Do not batch tiny commands into a different unit. A file's arm time is the median of its three calls.

Before timing, run an untimed complete preflight on every file and all four CoreAV arms. Require the expected backend (`libvpx` for VP9, `dav1d` for AV1), encoder, stream maps and positive frame count, equal across the hosting pair. Independently decode the complete paired outputs with pinned FFmpeg to frame hashes and compare timestamps, dimensions and every decoded sample per encoder. Compare copied-audio packet hashes too. A mismatch stops the diagnosis before timing. Preserve these checks separately from wall-clock samples. Use one small generated fixture for a tooling smoke test before the full remote job.

## Controls and reporting

Use the gate input identified by SHA-256 in the attribution's `host.txt` and in `inputs.json`. Before the first timed file and after the last, run B5's baseline on its default video stream with `-an`, one untimed warm-up followed by three timed calls. Void the whole run if the after/before median differs from 1 by more than 5%. Retain both groups of raw gate times. Report the A/A ratio and its interval without treating it as an additional acceptance bar.

For each class and each arm, report the median and a paired-file bootstrap 95% interval, 10,000 resamples with seed 518. Report seconds and ratios over the same files. CoreAV/baseline uses the file's `base` median as denominator. Also report `base2/base` and, separately per encoder, in-process/sandbox, the absolute seconds saved and the fraction of the sandbox's extra seconds over baseline removed. A fraction is undefined when that file's sandbox time is at or below baseline, and is reported apart rather than clipped. Report the count and share of files below 1.00 beside every ratio. These intervals describe these 10 and 6 previously timed files, not a held-out claim or a policy verdict.

Retain sanitized per-call wall times, the full input roster, preflight receipts, gate times and provenance. Report all failures and completeness. A missing call, duplicate call, wrong backend, changed input or failed gate prevents a valid summary. Preserve an incomplete report even when a subprocess fails. Raw private logs are not public evidence.

A per-stage profile is optional and must be separate from timing. The primary comparison removes exactly the worker path while retaining the decoder library and encoder. Do not instrument the hot path or infer stage CPU from a parent that may not have reaped its worker. If a reliable function-only profile is not cheap, omit it and state that the measurement attributes the total hosting-path effect, not an individual copy's cost.
