# Composed Opus-to-AAC check, version 2

This registration precedes its implementation and execution. [composed-opus-aac-v2.json](composed-opus-aac-v2.json) registers `composed-opus-aac-v2`. Version 1 and all frozen B9 inputs and results remain byte-for-byte unchanged. No version 1 receipt, cache entry, shard or stored result becomes version 2 evidence.

## Changed obligation: the source owns presentation

Version 2 replaces only the ordinary source-to-AAC baseline's authority over presentation duration and sample mapping. That baseline still supplies exact codec, profile, rate, channel count and start properties. It cannot prescribe a longer presentation by retaining source packet padding.

The independent source reference owns the presented sample count, rate, channels and origin. For MP4, bind the selected track and packet positions to source bytes, compute complete decoded work from Opus TOCs and `dOps` pre-skip, acquire the complete libopus decode with edits ignored, and select the presentation using only the source edit list. Subtract the pre-skip already removed by that decode when addressing its PCM; never apply it twice. The candidate or captured length must not choose the reference window.

For Matroska/WebM, retain the selected source's libopus presentation after its `CodecDelay` and `DiscardPadding`. This is consistent with [Matroska remux edit lists](mkv-remux.md#timing): the copied presentation excludes both priming and terminal discard rather than presenting the padded final frame. It grants no new packet-copy exception and does not change any copy predicate.

Require the source-derived count, rate, channels and origin to equal the capture binding and candidate presentation. Independently read every candidate AAC packet timestamp and the output edit list, remove its own priming, and require contiguous coverage of exactly that source count with no displacement, hole, overlap or interior timestamp change. Unsupported, missing or inconsistent source mappings fail. Nonzero origins remain unsupported rather than passing an endpoint-only check.

## Specification and observed difference

ISO/IEC 14496-12 section 8.6.6 makes `segment_duration` the media edit's presentation duration. [Opus in ISOBMFF 0.6.8](https://opus-codec.org/docs/opus_in_isobmff.html#4.4), section 4.4, specifies sample-accurate removal of startup and terminal padding through the edit list: at 48,000 movie and media ticks per second, `segment_duration` is the valid sample count and `media_time` the priming count. Section 4.3.2 leaves whole-playback priming removal to the edit list; `PreSkip` is not a second whole-playback trim.

The existing [decoded-source decision](transcode.md#decoded-audio-source-correctness-bars) records a generated two-second Opus MP4 with media time 312, pre-skip 312 and segment duration 96,000 at 48 kHz. The pinned FFmpeg's ordinary decode and AAC encode keep 96,648 samples. The version 1 Linux control likewise reads 96,648 presented AAC samples against 96,000 captured samples: 97,672 packet-duration samples minus 1,024 encoder priming samples. The 648-sample difference is terminal padding, not timestamp rounding. This agrees with the source-end rule of [KD-A-EDIT-END](../known-divergences-decode-audio.md#kd-a-edit-end); it does not extend that registry's scope or rewrite its historical evidence.

## Unchanged exact encoder and decoder bars

Every scope limit, capture boundary, resource bound, decoder conformance bar, comparator pin, sample projection, invocation expectation, publication rule and required negative test in [version 1](composed-opus-aac-v1.md) remains required except for the baseline timeline authority explicitly replaced above. Lossless and other source codecs retain their end-to-end exact check. No production decoder, encoder or timing behavior changes as part of this predicate version.

Replay the exact successfully written captured PCM with the independently expected pinned AAC command. Require exact ordered packet payloads, codec configuration and complete presented decoded F32. Candidate and exact-PCM replay must have the same priming; the replay starts at zero. Different raw media offsets in an ordinary baseline are not an error if their effective presentation agrees, and raw baseline priming equality is not required.

No tolerance, candidate-length trimming, alignment, replacement fixture or padding repair is permitted. Version 2 may accept the correct 96,000-sample source presentation without accepting the ordinary baseline's 648 extra samples.

## Version boundaries and verification

Before any command runs, determine command applicability from the source's stream metadata. The first-audio mapping case `x15` applies only when that metadata contains an audio stream selectable by its mandatory `0:a:0` map. Otherwise, record the excluded count and a reason for each fixture, outside the output-quality judgments. Neither a candidate nor an oracle outcome can establish inapplicability. An applicable fixture that fails remains counted as a failure.

After this registration commit, change every active selection, embedded capture identity, judging-source identity, fixture/comparator cache key and workflow input filter to version 2. Unknown, mixed, stale or relabeled evidence fails acquisition, resume, export, summary and shard merge. The historical version 1 registration and every frozen judging input stay unchanged.

Before merge, use trusted generated inputs only. First demonstrate the Linux stereo MP4 positive control and normal-versus-captured identity, then demonstrate rejection of wrong PCM, lag, encoder settings, sample count, source/call/stream binding and interior timing, plus no output or receipt after a failed or unsupported capture. Require all three acquisition shards and their merge, the existing Opus conformance gate and full current-head CI. Review starts at this standalone registration commit. Fixture success is not a coverage, performance or release verdict.
