# AVI output

`coreav ffmpeg` writes `.avi` and `-f avi` as a single AVI 1.0 RIFF file. The `coreav-avi` library plans stream headers, media chunks and an `idx1` index before writing. It accepts packet metadata rather than a source container. Its callback supplies exactly the planned payload bytes. Neither seeking nor retaining the complete media is required.

## Supported output

The CLI reads MP4 or MOV and supports copied H.264, MP3 and integer PCM. H.264 keeps its AVC configuration record and length-prefixed samples. Its frame cadence must be constant, without reordering. PCM is mono or stereo unsigned 8-bit or signed little-endian 16-bit integer audio. Wider samples and additional channels need a different WAVE format header and are refused.

`-c:a pcm_s16le` and `-c:a pcm_u8` decode audio through CoreAV. `-c:a libmp3lame` sends decoded f32le samples to the operator's FFmpeg program through the existing sandboxed encoder transport. `COREAV_FFMPEG` selects that program. It receives only raw samples on standard input and emits only MP3 on standard output. CoreAV validates complete MP3 frames and writes the AVI itself. Strict-safe mode refuses this external encoder.

AVI defaults to MP3 audio when no audio codec is selected. Its default MPEG-4 Part 2 video encoder is not implemented. An omitted video codec is never interpreted as stream copy. HuffYUV and FFV1 encoding are not implemented by this change.

## Refusals and bounds

The writer refuses output at or above 1 GiB, including headers, padding and the index. OpenDML, AVIX chunks and extended indexes are not implemented. The smaller bound is conservative, not a statement about RIFF's 32-bit size field.

There are at most sixteen output streams and one million packets. The index is limited to 16 MiB, configuration records to 1 MiB and each packet to 16 MiB. Decoded audio is limited to 64 MiB across retained streams. One external MP3 encode retains at most 16 MiB of compressed output and has a 120-second deadline.

Timestamp gaps, nonzero starts, video reordering, variable video cadence, source fragments, changing sample descriptions, rotation and edits that do not fit the fixed zero-based clock are refused. So are trimming, concat input, explicit metadata mapping, unsupported codecs and video encoder options. Audio encoding applies the decoder's presentation edits and refuses decoding warnings. Unsupported cases do not replace an existing output.

## Format rules

The layout follows Microsoft's [AVI RIFF File Reference](https://learn.microsoft.com/en-us/windows/win32/directshow/avi-riff-file-reference), [AVIStreamHeader](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/api/avifmt/ns-avifmt-avistreamheader), [AVIMAINHEADER](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/api/aviriff/ns-aviriff-avimainheader) and [MPEGLAYER3WAVEFORMAT](https://learn.microsoft.com/en-us/windows/win32/api/mmreg/ns-mmreg-mpeglayer3waveformat).

Each stream has an exact rational clock. Video and MP3 have one complete frame per chunk. PCM chunks contain whole interleaved sample frames. Chunk sizes exclude headers and odd-byte padding. Index offsets are relative to the `movi` FOURCC, so the first chunk is at offset four. All arithmetic and narrowing are checked during planning. Payload callbacks cannot write more or fewer bytes than planned.

AVI does not inherit MP4's gapless audio edits. External MP3 keeps the encoder's complete frames. Fixture verification compares complete decoded samples, stream fields and timing against the pinned FFmpeg running the same command. V2 coverage is reported only by the registered command oracle, never from encoded hashes or from these implementation limits.

## Implementation trace

| Choice | Source and consequence |
| --- | --- |
| `RIFF AVI `, `LIST hdrl`, `LIST movi`, then `idx1` | Microsoft's AVI RIFF File Reference, "AVI RIFF Form", requires this order. The plan emits it without seeking. |
| Little-endian sizes, four-byte list types and one zero pad byte for odd chunks | The same reference's "RIFF File Format" defines size exclusions and WORD alignment. Header and index sizes exclude padding. |
| `avih` fields and zero reserved words | Microsoft's AVIMAINHEADER defines the 56-byte payload, stream count, geometry, maximum chunk buffer and `AVIF_HASINDEX`. The descriptive microsecond period is rounded from the exact stream clock. Unused rate, skew and padding hints are zero. |
| `strh` fields, stream order and exact clocks | Microsoft's AVIStreamHeader defines scale/rate, length, sample size and destination rectangle. H.264 has one chunk per frame. PCM length is a count of complete sample frames. Four 16-bit rectangle values are serialized explicitly rather than using a native RECT layout. |
| PCM tag, block alignment and byte rate | Microsoft's [WAVEFORMATEX](https://learn.microsoft.com/en-us/previous-versions/dd757713(v=vs.85)) defines PCM tag 1, channels, sample rate, bits per sample, channel-frame alignment and bytes per second. CoreAV's `PcmFormat` determines signedness, endianness and integer layout. Other representations are refused. |
| MP3 tag and extra format fields | Microsoft's MPEGLAYER3WAVEFORMAT defines tag 0x55, twelve extra bytes, MPEG ID 1, ISO padding mode 0, a compressed-frame block, one frame per block and unknown delay 0. CoreAV's existing `mp3::Header` derives sample rate, channels, decoded frame samples and exact compressed frame sizes. |
| MP3 frame clock and AVC-in-AVI representation | These interoperability choices are gated by `avi-output` against the pinned binary. MP3 uses one frame per chunk, a zero fixed sample size and frame-sample scale/rate. H.264 keeps its AVC configuration record and packet representation. The fixture check compares complete decodes and exact stream/frame timing rather than encoded-file identity. |
| Sixteen-byte index entries, decimal stream numbers, keyframe flags and offsets from `movi` | Microsoft's [AVIOLDINDEX](https://learn.microsoft.com/en-us/previous-versions/ms779634(v=vs.85)) defines each entry and one entry per media chunk. Offset four points at the first chunk header. |
| No transition to AVIX or extended indexes | [OpenDML 1.02 section 2](https://jmcgowan.com/odmlff2.pdf) describes increased file sizes and RIFF-AVIX continuation. This writer implements neither. It refuses total output at or above 1 GiB before writing, rather than switching representations. |
| Packet roster, output atomicity and encoder lifecycle | CoreAV's packet APIs own source byte ranges and edited media ticks. `coreav_audio::decode` owns decoded presentation edits. `output::to_file` owns destination replacement. `encoder::Process` owns pipes, deadlines and child cleanup. AVI adds no separate subprocess lifecycle. |
| Resource limits and timing refusals | These are explicit implementation bounds, not format requirements. A validated plan checks chunk/index/file sizes before output. The adapter checks complete source presentation windows and fixed clocks, and refuses anything ordinary AVI cannot represent. |
