# PCM output through coreav ffmpeg

`coreav ffmpeg` writes signed 16-bit little-endian PCM to WAV or raw `-f s16le` files. WAV selects `pcm_s16le` by default. Raw output requires `-f s16le`; a `.raw` extension alone does not identify a sample format.

## Scope

The input uses the existing audio decoder for FLAC tracks in MP4, MOV and Matroska. Native PCM output accepts only decoded lossless sources, currently FLAC. PCM and ALAC input decoding are not implemented by this audio decoder. MP3's recorded agreement bar permits a floating-point tolerance, so it does not prove exact sample agreement with the pinned oracle. MP3, Opus and AAC sources retain their existing compatibility-command refusal and external FFmpeg fallback; strict-safe mode refuses them. Their `decode-audio` and lossy-transcode support is unchanged.

Automatic selection chooses one audio stream with the existing disposition and channel-count rules, without selecting video. An explicit subtitle codec still selects a subtitle and makes PCM output unsupported; `-sn` disables subtitle selection. Explicit maps must select exactly one audio stream. Output uses the source rate and channel layout.

The WAV writer reuses `coreav_audio::WavSink`. Raw output writes the same interleaved sample bytes without a header. Both outputs use the CLI's temporary-file replacement, including `-y`, `-n` and same-file protection. No external encoder runs, including in strict-safe mode.

Output `-threads` is accepted without changing PCM serialization or starting encoder workers. Other encoder options remain unsupported.

Resampling, channel mixing, raw input, multiple outputs, metadata copying, nonzero seeking and PCM in other containers are not covered. `-ar` is accepted only at the source rate. `-ac` is accepted only for an unchanged mono or stereo count. WAV refuses inputs with non-bookkeeping global tags unless `-map_metadata -1` is given, and refuses chapters. Raw output carries no tags. Unsupported commands are refused rather than silently discarding their options. A zero seek and a duration limit that is a whole sample count can select the start of the decoded presentation. The decoder still reads the whole input; this adapter does not add packet seeking or early termination to its API.

The compatibility command does not suppress decoder warnings. An ignored edit list is a compatibility refusal. Other decoder warnings fail the command, and a WAV file beyond the RIFF size limit is refused. Neither case publishes the staged file.

Input seeking is supported only for FLAC at zero. FFmpeg's input `-ss 0` can change AAC priming even though its requested time is zero, so stateful-codec input seeking is refused. Output `-ss 0` keeps the decoded presentation. An input `-t 0` means unlimited input, an output `-t 0` means empty output, and an effective `-to 0` is a usage error. Within each input or output group, `-t` overrides `-to`.

## Correctness bars

These bars are committed before comparisons run. Generated, deterministic signed 16-bit samples are stored in lossless FLAC tracks in Matroska and MP4. For every supported test command, the raw output and the decoded WAV output must equal the input sample bytes, with no tolerance. The existing decoder's edit-list trimming remains part of the input presentation.

For the same commands, the pinned FFmpeg produces an independent output. Its decoded PCM must match byte for byte. Compare `codec_name`, `sample_fmt`, `sample_rate`, `channels`, `channel_layout`, `bits_per_sample`, `time_base`, `duration_ts` and `duration` from the pinned ffprobe where the container exposes them. Raw output is probed with its explicit format, channel count and sample rate. Encoder-identification tags and file size need not match. A mismatch fails the comparison and is reported, not excluded.

Malformed streams, unsupported options, unsupported codecs, an existing output without replacement permission and an input/output alias must never publish a partial output. Unit and CLI tests retain these checks in CI. This change makes no size or speed claim and runs no timing measurement.

## Decisions

`coreav_audio::Codec::is_lossless` owns the source classification, beside the build-aware `Codec::from_name`. The transcode path intentionally accepts lossy decoded sources under its different agreement bars, so its capability predicate cannot also enforce this PCM contract. PCM has no second codec-name list.

A reproduction of compatibility CI's generated two-second mono Opus fixture differs from the pinned FFmpeg at 19 of 96,000 signed 16-bit samples. Its float output already differs before serialization; each writer correctly quantizes its own decoded floats. The decoder algorithm was unchanged by this PCM implementation. Opus decoder parity is separate work, not a tolerance or exception in the PCM checker. An Opus-to-WAV/raw regression preserves refusal, atomic output handling and the ordinary external FFmpeg fallback.

PCM output is a separate command job, not an external audio encoder beside a video transcode. This keeps it available in builds without video backends and prevents an audio-only output from entering the MP4 transcode pipeline. Container defaults belong to stream selection, while explicit codec options keep their last-match-wins semantics.

For multichannel WAV, the adapter writes the speaker layout reported for the selected stream. MP4 FLAC can report `5.1(side)` while the audio decoder's default mask is `5.1`; changing the output mask preserves the source's channel positions without mixing samples. An explicit `-ac 6` also changes FFmpeg's output layout even when the source already has six channels. That channel-layout conversion stays outside this adapter's unchanged-count support.
