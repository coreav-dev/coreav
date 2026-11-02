# Matroska packet output

`coreav_mkv::write` writes Matroska and WebM from track descriptions and packet metadata. It does not decode, encode, demux or change payload bytes. The CLI does not use it yet.

The format comes from [RFC 8794](https://www.rfc-editor.org/rfc/rfc8794), [RFC 9559](https://www.rfc-editor.org/rfc/rfc9559), the [Matroska codec mappings](https://www.matroska.org/technical/codec_specs.html) and the [WebM container guidelines](https://www.webmproject.org/docs/container/). No other implementation is an input.

## Layout and ownership

The caller supplies `Track` and `Packet` slices. `plan` validates them and returns an immutable `Plan` that borrows those slices. `Plan::write` accepts any `Write` and calls a payload copier with each original packet index. The copier must write exactly the packet's declared size. An I/O or copier error leaves a partial output that the caller must discard.

The first pass computes every element size and offset. The file has an EBML header, a known-size Segment, SeekHead, Info, Tracks, Cues and known-size Clusters. SeekHead references Info, Tracks and Cues. Cues precede the Clusters, so seeking does not require downloading the media first. Fixed-width eight-byte index positions make their lengths independent of the positions they contain. A seek-and-patch writer was rejected because the transcode API accepts non-seekable output.

Each video keyframe starts a Cluster. Other Clusters target five seconds or five MiB; a packet larger than the byte target occupies a Cluster of its own. The signed 16-bit block timestamp range always takes precedence over those targets. Video Cues include visible keyframes only. Audio-only output indexes the first random-access packet of each Cluster. When no valid cue exists, Cues and its Seek entry are omitted rather than writing an empty required list.

## Time and trimming

`Packet::pts_ns` is the presentation time after CodecDelay subtraction. The writer adds CodecDelay exactly once before converting it to a stored block timestamp. `Options::timestamp_scale_ns` defaults to one millisecond. Conversion rounds to the nearest tick, with ties toward positive infinity. Negative block times are supported when they fit relative to Cluster timestamp zero. Matroska packet storage order is never changed, including H.264 B-frames with nonmonotonic presentation times. WebM refuses decreasing stored timestamps.

The caller supplies DefaultDuration only for an established cadence. An explicit different packet duration becomes BlockDuration. Nonzero DiscardPadding also requires a BlockGroup. A dependent BlockGroup uses ReferenceBlock zero, the explicitly permitted indication that its dependencies are unknown in RFC 9559 sections 5.1.3.5.5 and 10.4. The writer does not guess reference-frame timestamps. Other packets use SimpleBlock.

Positive DiscardPadding trims the end and negative padding trims the beginning. The trim cannot exceed the packet duration. Packet durations include the encoded tail; BlockDuration and Segment Duration exclude positive padding. When splitting a laced input, the caller applies positive padding to its last frame only and negative padding to its first frame only. CodecDelay is distinct from such trims. Segment Duration is the greatest presentation end after positive padding, expressed in Segment Ticks; it is omitted when a track's final presentation duration is unknown.

DocTypeVersion is 4. DocTypeReadVersion is 2, so older readers can play the blocks with a reduced feature set as RFC 9559 section 7 permits. Readers must support version 4 timing elements to preserve priming and padding exactly.

## Codec boundary

`CodecId` maps a codec to its Matroska identifier, media kind, private-data requirement and WebM eligibility. Block and index serialization do not match on codec IDs. Adding another codec therefore changes the mapping and any codec-specific initialization validation, not the muxer. Payloads and initialization bytes must already use the mapping's representation. For example, H.264 uses an AVC configuration record and length-prefixed access units, AAC uses AudioSpecificConfig and raw access units, and Opus uses OpusHead.

WebM permits VP8, VP9, AV1, Vorbis and Opus, and refuses H.264, AAC and the other Matroska-only mappings. Opus validates the header's channel count, pre-skip-derived CodecDelay and at least 80 ms SeekPreRoll. The writer does not create codec private data from an MP4 sample entry. That conversion belongs to source adapters.

## Bounds and verification

`WriteLimits` is the existing Matroska `Limits`. Before layout allocation, each packet is charged 512 bytes against `max_index_bytes`, and packet count is bounded by `max_block_work`. Each track is charged its strings, codec private bytes and 256 bytes of serialization overhead against `max_retained_bytes`. TrackEntry size and codec private bytes obey `max_element_bytes`, track count obeys `max_tracks`, and payload size obeys `max_packet_bytes`. Payloads are copied through a size-limited writer, not retained. The caller's borrowed input metadata is outside these allocation charges.

`tests/write.rs` checks output through the public probe, packet and seek APIs. It covers decode-order preservation, exact payload boundaries, Opus delay and both trim signs, variable duration, signed timestamp boundaries, multi-byte track numbers, missing duration, codec restrictions and resource refusals. `tests/write_oracle.rs` generates short H.264/AAC, H.264/Opus and VP9/Opus sources with the pinned FFmpeg, rewrites their packets, then compares decoded frames, decoded audio samples and selected ffprobe fields. CI supplies the shared oracle; without `COREAV_FFMPEG`, the generated-media test skips rather than using an unpinned program.
