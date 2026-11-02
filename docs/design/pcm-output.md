# PCM output through coreav ffmpeg

`coreav ffmpeg` writes signed 16-bit little-endian PCM to WAV or raw `-f s16le` files. WAV selects `pcm_s16le` by default. Raw output requires `-f s16le`; a `.raw` extension alone does not identify a sample format.

## Scope

The input uses the existing audio decoder for MP4, MOV, Matroska and WebM. Its supported codecs and build features are unchanged. Automatic selection chooses one audio stream with the existing disposition and channel-count rules, without selecting video. Explicit maps must select exactly one audio stream. Output uses the source rate and channel layout.

The WAV writer reuses `coreav_audio::WavSink`. Raw output writes the same interleaved sample bytes without a header. Both outputs use the CLI's temporary-file replacement, including `-y`, `-n` and same-file protection. No external encoder runs, including in strict-safe mode.

Resampling, channel mixing, raw input, multiple outputs, metadata copying, nonzero seeking and PCM in other containers are not covered. Unsupported commands are refused rather than silently discarding their options. A zero seek and a duration limit can select the start of the decoded presentation.

## Correctness bars

These bars are committed before comparisons run. Generated, deterministic signed 16-bit samples are stored in lossless FLAC tracks in Matroska and MP4. For every supported test command, the raw output and the decoded WAV output must equal the input sample bytes, with no tolerance. The existing decoder's edit-list trimming remains part of the input presentation.

For the same commands, the pinned FFmpeg produces an independent output. Its decoded PCM must match byte for byte. Compare `codec_name`, `sample_fmt`, `sample_rate`, `channels`, `channel_layout`, `bits_per_sample`, `time_base`, `duration_ts` and `duration` from the pinned ffprobe where the container exposes them. Raw output is probed with its explicit format, channel count and sample rate. Encoder-identification tags and file size need not match. A mismatch fails the comparison and is reported, not excluded.

Malformed streams, unsupported options, unsupported codecs, an existing output without replacement permission and an input/output alias must never publish a partial output. Unit and CLI tests retain these checks in CI. This change makes no size or speed claim and runs no timing measurement.

## Decisions

PCM output is a separate command job, not an external audio encoder beside a video transcode. This keeps it available in builds without video backends and prevents an audio-only output from entering the MP4 transcode pipeline. Container defaults belong to stream selection, while explicit codec options keep their last-match-wins semantics.
