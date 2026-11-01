# `coreav probe` output for MPEG transport streams, schema `coreav.probe-mpegts/2`

This is the reference for the output of `coreav probe` on an MPEG transport
stream (`docs/design/mpegts.md`). The machine-readable twin is
[`probe-mpegts-v2.schema.json`](probe-mpegts-v2.schema.json) (JSON Schema
2020-12); tests validate every output against it. ISO base media files print
[`coreav.probe/1`](probe-v1.md) and Matroska and WebM files
[`coreav.probe-matroska/1`](probe-matroska-v1.md).

`/2` replaces `coreav.probe-mpegts/1`, whose keys and types it keeps. The
order of `streams` changed meaning: `/1` listed each Program Map Table's
streams in table order, the tables in the order they were read; `/2` lists
them in the order `ffprobe` numbers them (below). For a file of several
programs a reader of `/1` that took a stream by its position would now
find another PID, so the version changed (`probe-v1.md`, "Adding a key
keeps `/1`"). A file of one program gives the same document in both.

A transport stream has no header to recognise, so the command offers a file
to this reader only once the Matroska reader (EBML header `1A 45 DF A3`) and
the ISO base media reader (a first box of one of thirteen types) have
declined it; the reader then finds the packet size (188, 192 or 204 bytes)
and the first packet itself, so a stream that starts after a prefix of other
bytes is read too.

The rules of `coreav.probe/1` apply: every key listed here is always present,
unknown values are `null`, exact values come first, codec code points are
numbers from the specification that defines them, and adding a key keeps
`/1`. The shared shapes (rational, bit rate) and the `profile` objects are
those of `coreav.probe/1`.

A **time** here is `{"ticks": integer, "timescale": 90000, "seconds":
number}`: transport stream timestamps are 33-bit values at 90 kHz (ISO/IEC
13818-1 2.4.3.7), so the ticks are whole numbers, not a rational. Timestamps
are placed on one timeline for the whole file, past the 33-bit wrap, so they
may be negative or above 2^33 (`docs/design/mpegts.md`, "Timestamps").

## Top level

| Key | Type | Meaning |
|---|---|---|
| `schema` | string | Always `coreav.probe-mpegts/2`. |
| `file.size` | integer | File size in bytes. |
| `container.packet_size` | integer | Bytes a stored packet takes: `188`, `192` (a BDAV arrival timestamp before each packet) or `204` (188 plus 16 parity bytes). |
| `container.first_packet` | integer | Offset of the first stored packet; non-zero when the stream starts after other bytes. |
| `container.timescale` | integer | Always `90000`. |
| `container.scanned` | boolean | `--scan-frame-sizes` was given, so each stream has a `scan`. |
| `container.programs` | array | Programs in Program Association Table order: `number` (`program_number`), `pmt_pid`, `pcr_pid`, `stream_pids` (the PIDs its Program Map Table lists, in its order). |
| `start` | time or `null` | The earliest stream start. |
| `duration` | time or `null` | From the earliest stream start to the latest stream end; for a file with more than one program, the longest such span within one program, because each program keeps a system time base of its own and a span across them means nothing. `null` when no stream's timing is known. |
| `streams` | array of stream | Streams in the order `ffprobe` numbers them: first every stream a table has named by the time the file completes its first Program Map Table after its first PAT (that table's elementary streams, and an Event Information Table read before it), in the order the file shows them, a table's streams in the order it lists them; then every other stream in the order the file first shows it from its start, a table that names it or a PES packet of it that is not padding, the order the tables list them breaking ties. The others are the elementary streams of the other tables, the Event Information Table, and the PIDs that carry PES packets no table declares (`epg`, which is not an elementary stream but which `ffprobe` lists as a data stream, so that the two readers agree on what a broadcast capture holds). Dropped streams are left out; a PID that two programs share is listed once; `pmt_index` keeps each stream's place in its own table. |
| `dropped_streams` | array | Streams left out because their codec is not allowed: `stream_index`, `pid`, `stream_type`, `codec`, `reason` (`codec_not_allowed`). By default none: the reader keeps every codec it names, because a transport stream's duration and stream list are derived from its streams. `--allow` narrows the list; `--strict` rejects the file instead. |
| `warnings` | array of strings | File-level notes: lost sync, a transport error indicator, a continuity gap, a discarded duplicate packet, a skipped section or PES packet, a Program Map Table that changed version, scrambled packets, a file that ends inside a PES packet, and a packet listing that stopped. One copy of each, up to 256. The probe reads the head and the tail of the file, so without `--scan-frame-sizes` it reports only what those hold; with it, the listing reads the whole file and its own notes join these. |

## Stream

| Key | Type | Meaning |
|---|---|---|
| `index` | integer | Position in `streams`. |
| `stream_index` | integer | Position among every stream the tables name, dropped ones included (`ffprobe`'s `index`). |
| `pid` | integer | The PID its packets carry. |
| `programs` | array of integers | The `program_number`s whose tables list it. |
| `pmt_index` | integer or `null` | Its place in the elementary stream loop of the first Program Map Table that declared it, so that the table's own order is in the document, which `streams` follows only for the first table; `null` for a stream no table declares, that is, the Event Information Table. |
| `stream_type` | integer | `stream_type` from the Program Map Table (ISO/IEC 13818-1 table 2-34). |
| `codec` | string | CoreAV's codec name, which `--allow` takes: `h264`, `hevc`, `vvc`, `mpeg1video`, `mpeg2video`, `mpeg4`, `aac`, `aac_latm`, `mpeg_audio`, `ac3`, `eac3`, `dts`, `bluray_pcm`, `opus`, `scte35`, `id3`, `dvb_subtitle`, `dvb_teletext`, `pgs`, `arib_caption`, `epg`; and `other` for a stream whose type and descriptors name no codec this reader knows, which `--allow` can neither name nor drop. The stream type, refined by the registration and DVB descriptors and, for a private stream no descriptor names, by its first PES packet. The sequence header tells MPEG-1 from MPEG-2 video, which the stream type gets wrong, so a stream dropped by `--allow` before its header is read is named by its type. |
| `kind` | string | `video`, `audio`, `subtitle` or `data`. |
| `language` | string or `null` | The ISO 639-2 code of the ISO 639 language descriptor (tag 0x0A). |
| `descriptors` | array | Its elementary stream descriptors: `tag` and `size` (bytes of the descriptor's data). |
| `format_identifier` | string or `null` | The four bytes its own descriptors give its format, as a four-character code: the registration descriptor's `format_identifier` (ISO/IEC 13818-1 2.6.8), else the `metadata_format_identifier` of its metadata descriptor (2.6.60), which is how HLS names ID3 timed metadata. |
| `config_size` | integer or `null` | Bytes of the codec configuration the in-band parameter sets make, in the form MP4 and Matroska carry it (an `avcC` or `hvcC` payload, or an AudioSpecificConfig). |
| `profile` | object or `null` | Code points from that configuration, with `type` and `name` as in `coreav.probe/1`. |
| `mpeg2_profile` | object or `null` | MPEG-1 and MPEG-2 video: `profile` and `level` from `profile_and_level_indication` (ISO/IEC 13818-2 table 8-1) and the `name` `ffprobe` gives the profile. |
| `start` | time or `null` | Its first presentation timestamp. |
| `duration` | time or `null` | From that timestamp to the end of its last access unit, read from the tail of the file: an audio unit ends its frame header's duration after its time, a video unit the smallest step between presentation times later, and any other unit (a subtitle, a metadata identifier, a codec this reader cannot name) no time at all, because the step between such units is not a duration. This is the value `ffprobe` reports as `duration_ts`; where its own estimate leaves the last unit out, the difference is the documented divergence KD-TS-END-ESTIMATE. |
| `scan` | object or `null` | With `--scan-frame-sizes`: what the packet listing counted, `null` without it. The listing's own counts, not a second duration estimate. `packets`: access units listed (one per PES packet for video and data streams, one per frame for audio whose frames carry their length). `bytes`: their bytes together. `first_pts`, `last_pts`: time or `null`, the earliest and latest presentation timestamps listed. `span`: time or `null`, `last_pts` minus `first_pts`, one access unit short of `duration`. `bit_rate`: bit rate or `null`, `bytes` × 8 ÷ the stream's `duration` (not ÷ `span`, which ends where the last unit starts), as in `coreav.probe/1`. A listing stopped by the file's structure or a limit keeps the units read before it, and says so in the top-level `warnings`. |
| `video` | object or `null` | Video streams: `width`, `height` from the parameter sets or sequence header, and `color` (`primaries`, `transfer`, `matrix`: ITU-T H.273 code points; `full_range`: boolean) from the same. `null` for a video stream whose parameters the probe never read (a scrambled stream, or an MVC video sub-bitstream, whose subset parameter sets give no size), where `coreav ffprobe` prints 0 by 0 as `ffprobe` does. Also present for a presentation graphics subtitle, whose presentation composition segment states the picture it is composed on; that is the stream's own size, not the video stream's. |
| `audio` | object or `null` | Audio streams, from the first frame header: `sample_rate`, `channels` (`null` when the header leaves them to the payload), `detail` (the AAC audio object type, the MPEG audio layer, 0 for AC-3 or 1 for E-AC-3). |

## `--scan-frame-sizes`

For a transport stream the flag runs the packet listing over the whole file
and fills each stream's `scan`. The listing reads every byte, so its
warnings join the document's: damage between the head and the tail the
probe reads is invisible without it. It does not produce
`video.scanned_size`,
which `coreav.probe/1` and `coreav.probe-matroska/1` have: the picture sizes
come from the parameter sets the probe already reads.

## ffprobe-compatible output

Use `coreav ffprobe -of json -show_format -show_streams`.
This is a separate output contract, described in
[`docs/design/compat-cli.md`](../design/compat-cli.md), not a mode of the
native probe schema.

### Stream order, and `-map` for a file of several programs

The native probe and `coreav ffprobe` streams follow the order above, in which
`ffprobe` numbers them, so `stream_index` (and `index` when no stream is
dropped) is `ffprobe`'s index and `ffmpeg -map 0:N` names the same stream
on both sides, for a file of several programs too. ISO/IEC 13818-1 2.4.4
makes a transport stream a set of programs, not a list of streams, and
gives no order for them, so this order is a reader's choice; it is derived
from the file's bytes alone: which Program Map Table the file completes
first after its PAT, and where each other stream first shows itself.
`pmt_index` keeps each table's own order, and `container.programs` each
program's streams in it.

## Errors

`coreav.error/1`, as for `coreav.probe/1`. A file no reader recognises keeps
the ISO base media reader's `unsupported_container`. The transport stream
reader's own rejections are `invalid_structure` (a structure it cannot skip,
and a file shaped like a transport stream of at most 16 MiB that names no
program), `limit` (a longer file whose first 16 MiB name no program is
`max_probe_bytes`), `codec_not_allowed` (`--strict`) and `io`. The `limit`
names are the fields of `coreav_mpegts::Limits`: `max_probe_bytes`,
`max_resync_bytes`, `max_programs`, `max_streams`, `max_packet_bytes`,
`max_buffered_bytes`, `max_listing_packets`.
