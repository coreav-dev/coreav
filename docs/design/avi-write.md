# AVI output

`coreav ffmpeg` writes `.avi` and `-f avi` as a single AVI 1.0 RIFF file. The `coreav-avi` library plans stream headers, media chunks and an `idx1` index before writing. It accepts packet metadata rather than a source container. Its callback supplies exactly the planned payload bytes. Neither seeking nor retaining the complete media is required.

## Supported output

The CLI reads MP4 or MOV and supports copied H.264, MP3 and integer PCM. H.264 keeps its AVC configuration record and length-prefixed samples under the `avc1` handler. Its frame cadence must be constant, without reordering, and fit an exact 600 Hz AVI copy clock. PCM is mono or stereo unsigned 8-bit or signed little-endian 16-bit integer audio. Wider samples and additional channels need a different WAVE format header and are refused.

`-c:a pcm_s16le` and `-c:a pcm_u8` decode audio through CoreAV. `-c:a libmp3lame` sends decoded f32le samples to the operator's FFmpeg program through the existing sandboxed encoder transport. `COREAV_FFMPEG` selects that program. It receives only raw samples on standard input and emits only MP3 on standard output. CoreAV validates complete MP3 frames and writes the AVI itself. Strict-safe mode refuses this external encoder.

AVI defaults to MP3 audio when no audio codec is selected. Its default MPEG-4 Part 2 video encoder is not implemented. An omitted video codec is never interpreted as stream copy. HuffYUV and FFV1 encoding are not implemented by this change.

## Refusals and bounds

The writer refuses output at or above 1 GiB, including headers, padding and the index. OpenDML, AVIX chunks and extended indexes are not implemented. The smaller bound is conservative, not a statement about RIFF's 32-bit size field.

There are at most sixteen output streams and one million packets. The index is limited to 16 MiB, configuration records to 1 MiB and each packet to 16 MiB. Decoded audio is limited to 64 MiB across retained streams. One external MP3 encode retains at most 16 MiB of compressed output and has a 120-second deadline.

Timestamp gaps, nonzero starts, video reordering, variable video cadence, source fragments, changing sample descriptions, rotation and edits that do not fit the fixed zero-based clock are refused. So are trimming, concat input, explicit metadata mapping, unsupported codecs and video encoder options. Audio encoding applies the decoder's presentation edits and refuses decoding warnings. Unsupported cases do not replace an existing output.

## Format rules

The layout follows Microsoft's [AVI RIFF File Reference](https://learn.microsoft.com/en-us/windows/win32/directshow/avi-riff-file-reference), [AVIStreamHeader](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/api/avifmt/ns-avifmt-avistreamheader), [AVIMAINHEADER](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/api/aviriff/ns-aviriff-avimainheader) and [MPEGLAYER3WAVEFORMAT](https://learn.microsoft.com/en-us/windows/win32/api/mmreg/ns-mmreg-mpeglayer3waveformat).

Each stream has an exact rational clock. MP3 has one complete frame per chunk. H.264 has one payload chunk followed by empty chunks for the remaining slots of the source frame's duration. These empty chunks are time-bearing, including those after the final payload, and have their own index entries. Expanded chunks count against packet, index and file limits before allocation. PCM chunks contain whole interleaved sample frames. Chunk sizes exclude headers and odd-byte padding. Index offsets are relative to the `movi` FOURCC, so the first chunk is at offset four. All arithmetic and narrowing are checked during planning. Payload callbacks cannot write more or fewer bytes than planned, and empty clock slots invoke no payload callback.

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

### Concrete codec fields

Microsoft's [BITMAPINFOHEADER video semantics](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapinfoheader) define positive compressed height, one plane, an implied decoded bit depth, a little-endian compression FOURCC and an image-size field. They do not mandate the following AVC interoperability values. The pinned binary's [generated copy fixture](https://github.com/coreav-dev/coreav/actions/runs/41450289102/job/125293519364) exposes them directly in `strh` and `strf` bytes.

| Emitted field | Value and provenance |
| --- | --- |
| Video `fccType`, `fccHandler` | `vids`, `avc1`. The type is Microsoft's AVIStreamHeader video type; the handler preserves the AVC representation observed in the pinned output. |
| Bitmap `biSize` | `40 + avcC.len()`. The pinned 64-by-48 fixture stores 83 here for a 40-byte bitmap header and 43 configuration bytes. Its `strf` chunk also includes a final alignment byte; this writer uses RIFF's external odd-byte padding instead. |
| `biWidth`, `biHeight`, `biPlanes` | Source coded width and positive height, with one plane. These follow BITMAPINFOHEADER video semantics. Geometry is checked before serialization. |
| `biBitCount`, `biCompression`, `biSizeImage` | 24, `avc1`, `width * height * 3`. These are an AVC bitmap-format convention, not an assertion that H.264 payloads are packed RGB. The pinned fixture stores 24, `avc1` and 9,216 for 64 by 48 pixels. Actual decoded pixel format and geometry come from the AVC configuration and must match in the complete decode check. |
| Bitmap density and palette fields | Both pixels-per-meter values and both color-count values are zero. No physical display density or palette is supplied for this compressed format. Microsoft's BITMAPINFOHEADER defines those fields; the pinned fixture also stores four zero words. |
| MP3 `nBlockAlign` and `dwSampleSize` | `frame_samples` and zero. This is the AVI frame-clock convention, not PCM byte alignment. MPEG-1 frames have 1,152 decoded samples and MPEG-2/2.5 frames have 576 according to CoreAV's MP3 header parser. Complete decoded equality is verified for generated output; the fixture artifact retains the pinned `strf` bytes separately so this format-header choice can be inspected directly. |
| MP3 extra block fields | MPEG ID 1, ISO padding mode 0, maximum compressed frame bytes as the block buffer size, one frame per block and delay zero. Microsoft's MPEGLAYER3WAVEFORMAT supplies the field definitions and the unknown-delay rule. The actual frame header supplies compressed byte counts. This does not remove encoder delay or trailing samples. |

### Byte-proven H.264 copy clock

The pinned 25 fps fixture has `strh.dwScale = 1`, `dwRate = 600`, `dwLength = 144` and `dwSampleSize = 0`. Its `movi` and `idx1` contain 144 video chunks. Six have payloads at slots 0, 24, 48, 72, 96 and 120. Every payload is followed by 23 zero-length chunks, including the final payload. Empty entries have flags zero and their offsets increase by eight bytes. The decoder reports six frames, each with duration 1/600, at the original presentation times, and the stream ends at 144/600 seconds.

The CLI reproduces this observed slot representation only when the source's fixed frame duration is an exact positive integer of 1/600 second. It refuses rounding, reordering, gaps and incomplete final presentation windows. The library's descriptive `avih` period is rounded from its actual stream clock; it does not copy the reference's unrelated 78-microsecond main-header hint. Stream and decoded-frame clocks remain the equality gate. This is a compatibility choice established by emitted bytes, not an inferred requirement of RIFF or a relaxed timing predicate.
